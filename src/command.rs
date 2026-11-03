//! Serializable tank commands and device-to-command translation.

use bevy::ecs::entity::{EntityMapper, MapEntities};
use bevy::ecs::lifecycle::Remove;
use bevy::math::Affine3A;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::damage::{CrewStation, TankKnockedOut};
use crate::firecontrol::Ranging;
use crate::spec::Trigger;
use crate::state::{GameplaySet, PlayerInputSet};
use crate::tank::{Controlled, Muzzle, TankRoot, Weapon, WeaponIndex};

/// One tick's driver intent for a tank.
#[derive(Component, Default, Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Reflect)]
#[require(ParsedTankCommand)]
pub struct TankCommand {
    /// Target throttle in [-1, 1]: forward/reverse drive.
    pub throttle: f32,
    /// Target steer in [-1, 1]: differential yaw, positive to the right.
    pub steer: f32,
    /// Primary fire edge, latched until one fixed tick consumes it.
    pub fire_primary: bool,
    /// Secondary fire level. It is a consumable and must be attested for the current tick.
    pub fire_secondary: bool,
    /// Aim intention chased by every servo, in the frame its authoring view is anchored to
    /// ([`AimIntent`]). `None` means no commitment yet.
    pub aim: Option<AimIntent>,
    /// Player-dialed range (m) for superelevation.
    pub range: f32,
    /// Crew-swap edge, validated against the tank's seats by simulation authority.
    pub crew_swap: Option<CrewSwap>,
    /// Round-selection edge, validated against the weapon's round list by simulation authority.
    pub select_round: Option<RoundSelect>,
    /// The game's damage model: toggle the selected round type's fuze mode, addressed by weapon.
    pub toggle_fuze: Option<u8>,
    /// Respawn edge; authority validates that this tank is knocked out.
    pub respawn: bool,
    /// The game's damage model: start or cancel module repair, consumed once.
    pub repair: bool,
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
        self.select_round = None;
        self.toggle_fuze = None;
        self.respawn = false;
        self.repair = false;
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
        self.fire_primary
            || self.crew_swap.is_some()
            || self.select_round.is_some()
            || self.toggle_fuze.is_some()
            || self.respawn
            || self.repair
    }

    /// The command of a tank whose death flow owns the device ([`DeviceOwner`]): the respawn edge
    /// and nothing else.
    fn neutral(self) -> Self {
        Self {
            respawn: self.respawn,
            ..default()
        }
    }
}

/// Why a raw input snapshot cannot enter simulation. These are outside-input refusals at the
/// network bridge; a locally authored invalid snapshot is a bug in its device/tool writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandError {
    Throttle,
    Steer,
    Range,
    Aim,
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Throttle => "throttle must be finite and within [-1, 1]",
            Self::Steer => "steer must be finite and within [-1, 1]",
            Self::Range => "range must be finite and nonnegative",
            Self::Aim => "aim must be finite and inside the derived AIM_LIMIT envelope",
        })
    }
}
impl std::error::Error for CommandError {}

/// One parsed input snapshot. Present with the raw command at spawn, initially idle; the input
/// boundary fills it before gameplay. Its fields are private so consumers cannot bypass parsing.
/// The servo bridge reads the checked aim; other sim readers receive its validated raw copy.
#[derive(Component, Default, Debug, Clone, Copy)]
pub(crate) struct ParsedTankCommand {
    command: TankCommand,
    aim: Option<ParsedAimIntent>,
    /// The tank's player commands nothing ([`Self::hold`]).
    held: bool,
}

impl TryFrom<TankCommand> for ParsedTankCommand {
    type Error = CommandError;
    fn try_from(command: TankCommand) -> Result<Self, Self::Error> {
        for (level, error) in [
            (command.throttle, CommandError::Throttle),
            (command.steer, CommandError::Steer),
        ] {
            if !level.is_finite() || !(-1.0..=1.0).contains(&level) {
                return Err(error);
            }
        }
        if !command.range.is_finite() || command.range < 0.0 {
            return Err(CommandError::Range);
        }
        let aim = command.aim.map(ParsedAimIntent::try_from).transpose()?;
        Ok(Self {
            command,
            aim,
            held: false,
        })
    }
}

impl std::ops::Deref for ParsedTankCommand {
    type Target = TankCommand;
    fn deref(&self) -> &Self::Target {
        &self.command
    }
}

impl ParsedTankCommand {
    /// The command of a tank whose player commands nothing: every level and edge idle, no aim, so
    /// its servos keep their targets, and the brakes held, so it brakes to a stop and stays parked
    /// (`track::drive::DriveIntent::Brake`).
    pub(crate) fn hold() -> Self {
        Self {
            held: true,
            ..default()
        }
    }
    pub(crate) fn held(self) -> bool {
        self.held
    }
    pub(crate) fn command(self) -> TankCommand {
        self.command
    }
    pub(crate) fn aim(self) -> Option<ParsedAimIntent> {
        self.aim
    }
    pub(crate) fn attest(mut self, tick: u32) -> Self {
        if self.command.for_tick != tick {
            self.command.fail_consumables_closed();
        }
        self
    }
}

/// An aim whose finite point is inside the derived authoring envelope, parsed once at input.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ParsedAimIntent(AimIntent);

impl TryFrom<AimIntent> for ParsedAimIntent {
    type Error = CommandError;
    fn try_from(intent: AimIntent) -> Result<Self, Self::Error> {
        let point = intent.point();
        if !point.is_finite() || point.abs().max_element() > *crate::aim::AIM_LIMIT {
            return Err(CommandError::Aim);
        }
        Ok(Self(intent))
    }
}
impl ParsedAimIntent {
    pub(crate) fn in_hull(self, to_local: &Affine3A) -> Vec3 {
        self.0.in_hull(to_local)
    }
}

/// Every tank's raw command, parsed before gameplay. A tank whose input arrives over a link is
/// parsed again by the net layer's input bridge, which orders after [`ParseCommands`], attests the
/// snapshot and replaces this parse. Its raw `TankCommand` holds only values this parse accepts:
/// on the server, the bridge's previous parsed value (edges cleared, or zeroed on release); on the
/// network client's own tank, the frame's device writers over it, the same writers an offline run
/// parses here.
pub(crate) fn parse_commands(mut commands: Query<(&TankCommand, &mut ParsedTankCommand)>) {
    for (&command, mut parsed) in &mut commands {
        *parsed = ParsedTankCommand::try_from(command)
            .expect("device/tool writers and the input bridge author finite bounded commands");
    }
}

/// The ordering handle of [`parse_commands`]: the net layer's input bridge runs after it.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParseCommands;

/// An aim intention and the frame it is measured in (ADR-0038).
///
/// **The intention travels in the frame its authoring view is anchored to**, and only the author
/// knows which that is — so the frame rides the wire with the point instead of being inferred.
///
/// A world-anchored view names a PLACE: the orbit camera does not turn with the hull, so the
/// player's pick is a spot on the ground, and the delivery gap must not rotate that spot out from
/// under them. A hull-anchored view names a BEARING off the tank: in the gunner optic and free-aim
/// the camera, the working yaw/pitch intent and the gun itself all ride the hull, so a hull-local
/// point is already invariant across the gap — naming it through the client's INTERPOLATED hull
/// only to have the authority decompose it against the TRUE hull would import the difference
/// between the two as noise in the fired lay.
///
/// Either way a HOLD decays hull-locally: the memory behind both (`aim::CommittedAim`) is
/// hull-local, so stopping picking never becomes stabilization (ADR-0001).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize, Reflect)]
pub enum AimIntent {
    /// A place on the world.
    World(Vec3),
    /// A bearing off the hull, as the point it names in the hull's frame.
    HullLocal(Vec3),
}

impl AimIntent {
    /// The raw point as authored, whichever frame it names. The input boundary parses it into
    /// [`ParsedAimIntent`] before any servo consumer; the frame tag is never a warrant.
    pub fn point(self) -> Vec3 {
        match self {
            Self::World(point) | Self::HullLocal(point) => point,
        }
    }

    /// The point this names in the hull's frame, given that hull's inverse.
    pub fn in_hull(self, to_local: &Affine3A) -> Vec3 {
        match self {
            Self::World(point) => to_local.transform_point3(point),
            Self::HullLocal(point) => point,
        }
    }

    /// The point this names in the world, given the hull's pose.
    pub fn in_world(self, hull: &Affine3A) -> Vec3 {
        match self {
            Self::World(point) => point,
            Self::HullLocal(point) => hull.transform_point3(point),
        }
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

/// One round-selection step for one weapon, addressed by its slot ([`WeaponIndex`], the name-sorted
/// order). The step is relative and wraps through the weapon's round list, so steps authored before
/// the first is confirmed each count; the authority applies it to the weapon's selection
/// (`tank::WeaponLoads`), which takes effect at the weapon's next load.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Reflect)]
pub struct RoundSelect {
    pub weapon: u8,
    pub step: RoundStep,
}

/// A step through a weapon's round list, wrapping at either end.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize, Reflect)]
pub enum RoundStep {
    Next,
    Previous,
}

impl RoundStep {
    /// The index one step from `from` in a list of `count` rounds, wrapping; `from` past the end is
    /// first taken modulo `count`. `None` for an empty list or a result past `u8`.
    pub fn apply(self, from: u8, count: usize) -> Option<u8> {
        if count == 0 {
            return None;
        }
        let from = usize::from(from) % count;
        let to = match self {
            Self::Next => (from + 1) % count,
            Self::Previous => (from + count - 1) % count,
        };
        u8::try_from(to).ok()
    }
}

// `TankCommand` has no `Entity` fields (`aim`/`range` are plain data, `crew_swap` addresses seats
// by `CrewStation` and `select_round` weapons by slot, not entity id) — lightyear's native input plugin requires `MapEntities` on the
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
    /// Toggles the pen cam's x-ray: the player's own tank in the glass look on the
    /// battlefield, the latest shot that struck it drawn over it (`damage_cam::pen_cam::game`). A
    /// view, not a command: it never reaches [`TankCommand`].
    pub xray: ButtonBinding,
    /// The kill cam, while the player's own tank is knocked out: hold the clock while down, replay
    /// from the death screen. Views, not commands: neither reaches [`TankCommand`]. Read only
    /// while the death flow owns the device ([`DeviceOwner`]), so they may share a key with a tank
    /// binding. Esc skips the run (`state::EscapeAction`), unbound here.
    pub kill_cam_hold: ButtonBinding,
    pub kill_cam_replay: ButtonBinding,
    /// Step the main gun's (the `Primary` trigger's weapon) round selection forward or back
    /// through its round list ([`RoundSelect`]); the selection is loaded at the next load.
    pub round_next: ButtonBinding,
    pub round_previous: ButtonBinding,
    pub fuze_mode: ButtonBinding,
    pub repair: ButtonBinding,
}

/// Who the player's device speaks for.
///
/// While the death flow (the death screen and the kill cam) is active for the controlled tank it
/// owns the device: [`TankDeviceSet`] stands aside, so no drive, aim, fire, sight or crew swap
/// command is taken from the device for that tank, and the respawn, map and kill cam keys are the
/// death flow's alone. [`claim_device`] decides it from the controlled tank's knockout fact
/// (`TankKnockedOut`) in [`DeviceClaim`] and, while the death flow owns the device, writes the
/// [neutral](TankCommand::neutral) command, so the frame's [`DeviceCommand`] is neutral. It hands
/// the device back on the first claim that finds a live tank controlled, or none (a respawn, a
/// disconnect). Not a capability gate: the sim never reads it.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
enum DeviceOwner {
    #[default]
    Tank,
    DeathFlow,
}

/// The systems that turn the player's device into the controlled tank's commands and view: the
/// gather, the range dial, the aim commits, the sight toggle, the crew swap and the trigger click.
/// They run only while the tank owns the device ([`DeviceOwner`]).
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct TankDeviceSet;

/// Where [`claim_device`] runs: in `RunFixedMainLoop` before [`TankDeviceSet`], and in
/// `FixedPreUpdate`. A knockout fact realized before the frame's ticks (the network client's
/// replicated status) orders before the frame claim, so the device stands aside and the frame's
/// [`DeviceCommand`] is neutral from the frame the fact arrives in. One latched inside a tick (the
/// offline sim, whose input is [`TankCommand`] itself) is claimed by the next tick, so the frame's
/// later ticks read the neutral command; the latching tick read its command before the fact
/// existed. On the network client the input bridge rewrites [`TankCommand`] before any sim reader,
/// so the tick claim changes nothing there.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct DeviceClaim;

/// The controlled tank's command as the device said it this frame: taken once per frame after
/// [`TankDeviceSet`] and the frame's [`DeviceClaim`], so it is neutral while the death flow owns
/// the device, and default when no tank is controlled. The network client's feed files it on every
/// tick of the frame and clears its edges after the first, so the wire carries the device's command
/// and each edge once. [`TankCommand`] is the sim's input: on the network client the input bridge
/// overwrites it every tick with lightyear's restored input, which never reaches the wire.
///
/// Invariant: the edges it takes are the device's alone, because [`consume_edges`] clears every
/// edge the bridge restored by the end of its tick; the levels are re-authored every frame (the
/// gather, and the aim commit per `aim::CommittedAim`).
#[derive(Resource, Default, Clone, Copy, PartialEq, Debug)]
pub(crate) struct DeviceCommand(pub(crate) TankCommand);

/// Take the frame's [`DeviceCommand`] from the controlled tank's command.
fn snapshot_device_command(
    mut device: ResMut<DeviceCommand>,
    own: Query<&TankCommand, With<Controlled>>,
) {
    // No controlled tank (before spawn, between possessions) is the idle command.
    device.0 = crate::tank::controlled_one(own.single())
        .copied()
        .unwrap_or_default();
}

/// Decide [`DeviceOwner`] from the controlled tank's knockout fact and, while the death flow owns
/// the device, write the controlled tank's command [neutral](TankCommand::neutral). In
/// [`DeviceClaim`]. Absent or knocked-out bodies discard their persistent levers.
fn claim_device(
    mut owner: ResMut<DeviceOwner>,
    mut drive_input: ResMut<DriveInput>,
    mut own: Query<(&mut TankCommand, Has<TankKnockedOut>), With<Controlled>>,
) {
    let dead = own.iter().any(|(_, knocked_out)| knocked_out);
    owner.set_if_neq(if dead {
        DeviceOwner::DeathFlow
    } else {
        DeviceOwner::Tank
    });
    if dead || own.is_empty() {
        drive_input.retain_for(None);
    }
    if dead {
        for (mut command, _) in &mut own {
            let neutral = command.neutral();
            command.set_if_neq(neutral);
        }
    }
}

/// Whether the controlled tank owns the player's device ([`DeviceOwner`]).
fn tank_owns_device(owner: Option<Res<DeviceOwner>>) -> bool {
    owner.is_none_or(|owner| *owner == DeviceOwner::Tank)
}

/// A [-1, 1] axis from a key pair.
pub struct AxisKeys {
    pub pos: KeyCode,
    pub neg: KeyCode,
}

/// Client-only persistent levers. Ownership changes discard both axes so a respawn or Tab swap
/// cannot inherit a previous tank's cruise command. Repair brakes mask the request without
/// moving these levers. The game's damage model: an empty Driver station also masks the
/// request without discarding it; staffing resumes the latest notch, including taps made while absent.
/// No field is replicated.
#[derive(Resource, Default)]
pub(crate) struct DriveInput {
    owner: Option<Entity>,
    throttle: AxisLever,
    steer: AxisLever,
    effective: crate::track::drive::DriveAxes,
}

impl DriveInput {
    fn retain_for(&mut self, owner: Option<Entity>) {
        if self.owner != owner {
            *self = Self { owner, ..default() };
        }
    }

    pub(crate) fn notches(&self) -> crate::track::drive::DriveAxes {
        self.effective
    }
}

#[derive(Default)]
struct AxisLever {
    notch: f32,
    press: Option<LeverPress>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LeverDirection {
    Positive,
    Negative,
}

impl LeverDirection {
    fn sign(self) -> f32 {
        match self {
            Self::Positive => 1.0,
            Self::Negative => -1.0,
        }
    }
    fn from_buttons(positive: bool, negative: bool) -> Option<Self> {
        match (positive, negative) {
            (true, false) => Some(Self::Positive),
            (false, true) => Some(Self::Negative),
            _ => None,
        }
    }
}

struct LeverPress {
    direction: LeverDirection,
    started_at: f64,
}

impl AxisLever {
    fn sample(&mut self, binding: &AxisKeys, keys: &ButtonInput<KeyCode>, now: f64) -> f32 {
        let feel = crate::track::drive::drive_feel();
        let (pos, neg) = (keys.pressed(binding.pos), keys.pressed(binding.neg));
        // Opposite keys together are a deliberate neutral command, cancelling the lever.
        if pos && neg {
            *self = Self::default();
            return 0.0;
        }
        let direction = LeverDirection::from_buttons(pos, neg);
        if let Some(press) = &self.press {
            let repressed = if press.direction == LeverDirection::Positive {
                keys.just_released(binding.pos) && keys.just_pressed(binding.pos)
            } else {
                keys.just_released(binding.neg) && keys.just_pressed(binding.neg)
            };
            if direction != Some(press.direction) || repressed {
                // A hold that ends between gather frames still clears its notch on release.
                if now - press.started_at >= feel.tap_window_s.0 {
                    self.notch = 0.0;
                }
                self.press = None;
            }
        }
        if self.press.is_none() {
            // Both edges may arrive in one frame: a sub-frame tap still steps exactly once.
            let tapped = LeverDirection::from_buttons(
                keys.just_pressed(binding.pos),
                keys.just_pressed(binding.neg),
            );
            if let Some(start) = direction.or(tapped) {
                self.notch = feel.tap_notch(self.notch, start == LeverDirection::Positive);
                self.press = direction.map(|direction| LeverPress {
                    direction,
                    started_at: now,
                });
            }
        }
        match &self.press {
            Some(press) if now - press.started_at >= feel.tap_window_s.0 => press.direction.sign(),
            _ => self.notch,
        }
    }
}

/// One bindable button — keyboard or mouse, so "fire" can live on either.
#[derive(Clone, Copy, Debug)]
pub enum ButtonBinding {
    Key(KeyCode),
    Mouse(MouseButton),
}

impl ButtonBinding {
    pub(crate) fn pressed(
        &self,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
        match *self {
            Self::Key(key) => keys.pressed(key),
            Self::Mouse(button) => mouse.pressed(button),
        }
    }

    /// `pub(crate)` for `sfx`: the trigger click reads the device edge directly, because the
    /// command's `fire_primary` is a latch a fixed tick consumes, not a per-frame edge.
    pub(crate) fn just_pressed(
        &self,
        keys: &ButtonInput<KeyCode>,
        mouse: &ButtonInput<MouseButton>,
    ) -> bool {
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
            // TUNED: the x-ray's key.
            xray: ButtonBinding::Key(KeyCode::KeyX),
            // TUNED: the kill cam's keys.
            kill_cam_hold: ButtonBinding::Key(KeyCode::KeyC),
            kill_cam_replay: ButtonBinding::Key(KeyCode::Space),
            // TUNED: the round selector's keys.
            round_next: ButtonBinding::Key(KeyCode::KeyE),
            round_previous: ButtonBinding::Key(KeyCode::KeyQ),
            // ASSUMED minimal fuze-mode binding.
            fuze_mode: ButtonBinding::Key(KeyCode::KeyV),
            repair: ButtonBinding::Key(KeyCode::KeyF),
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
    crate::track::drive::drive_feel();
    app.add_systems(
        FixedUpdate,
        parse_commands.in_set(ParseCommands).before(GameplaySet),
    );
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
        .init_resource::<DeviceOwner>()
        .init_resource::<DriveInput>()
        .configure_sets(RunFixedMainLoop, TankDeviceSet.run_if(tank_owns_device))
        .configure_sets(Update, TankDeviceSet.run_if(tank_owns_device))
        .configure_sets(
            RunFixedMainLoop,
            DeviceClaim
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .before(TankDeviceSet),
        )
        .init_resource::<DeviceCommand>()
        .add_systems(RunFixedMainLoop, claim_device.in_set(DeviceClaim))
        .add_systems(FixedPreUpdate, claim_device.in_set(DeviceClaim))
        .add_systems(
            RunFixedMainLoop,
            snapshot_device_command
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .after(TankDeviceSet)
                .after(DeviceClaim),
        )
        // Once per render frame, before the fixed loop runs its 0..N sim ticks — so every tick
        // this frame sees the same, freshest command, and edge inputs latch here without being
        // missed or doubled by the fixed clock.
        .add_systems(
            RunFixedMainLoop,
            (gather_commands, select_round_input)
                .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
                .in_set(PlayerInputSet)
                .in_set(TankDeviceSet)
                .in_set(GameplaySet),
        );
}

/// The scripted-trigger hook, as its OWN mount — the offline capture composition (`run_offline`)
/// is the only caller, and that is a security boundary, not a tidiness preference.
///
/// [`client_plugin`] above is shared: `NetClientPlugin` mounts it too, and every build, the
/// packaged release client included, compiles `auto_fire`'s code. Had the registration stayed in
/// the shared mount, any player could launch the shipped client with `SPIKE_AUTO_FIRE=1` and have
/// scripted trigger edges written into `TankCommand` — which the net input bridge then sends to a
/// LIVE server as genuine player input. Registering it here instead makes the hook STRUCTURALLY
/// absent from every network-client app: there is no env var, no config and no build flag that can
/// reach it, because the systems are never added to that `App` at all.
///
/// Two tests pin the boundary in both directions: the shared mount never arms the trigger even in
/// an armed app (`the_shared_device_gather_never_arms_auto_fire_even_when_armed`),
/// and `run_offline` is the crate's ONLY mount site
/// (`offline_auto_fire_plugin_is_mounted_only_inside_run_offline`, a source scan — "no other plugin
/// mounts this" is not expressible in the type system).
pub fn offline_auto_fire_plugin(app: &mut App) {
    let armed = crate::env_flag("SPIKE_AUTO_FIRE", false).unwrap_or_else(crate::refuse_start);
    #[cfg(test)]
    let armed = app
        .world()
        .get_resource::<AutoFireArmed>()
        .map_or(armed, |armed| armed.0);
    if !armed {
        return;
    }
    info!("auto_fire: armed — {AUTO_FIRE_SCHEDULE}");
    app.add_systems(
        RunFixedMainLoop,
        auto_fire
            // `gather_commands` comes from [`client_plugin`], which the offline root also mounts
            // (via `ClientPlugin`): the override must land after the real devices have written.
            .after(gather_commands)
            .in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop)
            .in_set(PlayerInputSet)
            .in_set(TankDeviceSet)
            .in_set(GameplaySet),
    );
}

/// Whether [`offline_auto_fire_plugin`] arms the scripted trigger. Read once, at that mount: an
/// `App` already carrying it is taken at its word, and otherwise `SPIKE_AUTO_FIRE` decides. Nothing
/// in production inserts it.
#[cfg(test)]
#[derive(Resource, Clone, Copy)]
struct AutoFireArmed(bool);

/// The hardcoded auto-fire timeline, in `Time<Real>` seconds — the SAME clock `frame_cost` stamps
/// its rows with, so a capture can be split into windows by these boundaries alone. Stated as one
/// string because it is logged verbatim at arm time: the capture's own record of what it drove.
const AUTO_FIRE_SCHEDULE: &str =
    "idle 0-20s, MG held 20-50s, idle 50-60s, main gun 60-75s, idle 75s+";

/// Dev-only scripted trigger (`SPIKE_AUTO_FIRE=1`): drive the controlled tank's triggers off a
/// hardcoded wall-clock schedule so a frame capture contains an idle window and a sustained-fire
/// window measured in ONE session — comparing firing cost against a same-session idle baseline is
/// the whole point, and a human holding the key would also be feeding the window mouse/keyboard
/// input the sweep's validity rules forbid.
///
/// Deliberately knob-free (the timeline is the constant above): this exists to answer one question,
/// not to become a scripting facility. It writes only the two trigger fields, after
/// [`gather_commands`] has written the real devices, so it composes as an override rather than a
/// second input source.
fn auto_fire(
    time: Res<Time<Real>>,
    mut phase: Local<u8>,
    mut tanks: Query<&mut TankCommand, With<Controlled>>,
) {
    let t = time.elapsed_secs();
    // Phase index over the schedule above; logged on change so the capture log carries the
    // boundaries the analysis windows are cut on.
    let now = match t {
        _ if t < 20.0 => 0,
        _ if t < 50.0 => 1,
        _ if t < 60.0 => 2,
        _ if t < 75.0 => 3,
        _ => 4,
    };
    if now != *phase {
        info!("auto_fire: phase {now} at t={t:.3}s");
        *phase = now;
    }
    for mut command in &mut tanks {
        // The MG is a held level; the main gun a latched click edge the fire tick consumes, so
        // re-latching every frame simply fires as fast as the reload gate allows.
        command.fire_secondary = now == 1;
        command.fire_primary |= now == 3;
    }
}

/// Translate devices through the bindings into the controlled tank's command. The only place in
/// the sim path that reads a device. `pub(crate)` so the other `BeforeFixedMainLoop` command
/// writers (`firecontrol::adjust_range`, `sight::drive_gunner_aim`) can pin an explicit order
/// against it — both share the `Ranging`/`TankCommand` it touches. In [`TankDeviceSet`].
pub(crate) fn gather_commands(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Res<Bindings>,
    ranging: Res<Ranging>,
    time: Res<Time<Real>>,
    mut drive_input: ResMut<DriveInput>,
    mut tanks: Query<(Entity, &mut TankCommand), With<Controlled>>,
) {
    // No controlled body before spawn or between possessions: no persistent drive input.
    let Some((entity, mut command)) = crate::tank::controlled_one(tanks.single_mut()) else {
        drive_input.retain_for(None);
        return;
    };
    drive_input.retain_for(Some(entity));
    let now = time.elapsed_secs_f64();
    command.throttle = drive_input.throttle.sample(&bindings.throttle, &keys, now);
    command.steer = drive_input.steer.sample(&bindings.steer, &keys, now);
    drive_input.effective = crate::track::drive::DriveAxes {
        throttle: command.throttle,
        steer: command.steer,
    };
    // `|=`: a click must survive frames the fixed loop skips, until a tick consumes it.
    command.fire_primary |= bindings.fire_primary.just_pressed(&keys, &mouse);
    command.repair |= bindings.repair.just_pressed(&keys, &mouse);
    command.fire_secondary = bindings.fire_secondary.pressed(&keys, &mouse);
    // The dial itself (`Ranging`, scroll in the optic) is client-side control state; the
    // command carries its absolute value. `aim` is written by the per-view commit systems.
    command.range = ranging.range;
}

/// Latch a round-selection step for the controlled tank's weapon on the `Primary` trigger from the
/// `round_next`/`round_previous` bindings. Only the step goes on the command: the authority applies
/// it (`shooting::apply_round_selection`), and the HUD shows the replicated choice, never this
/// latch. Pressing both in one frame latches nothing. In [`TankDeviceSet`].
fn select_round_input(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Res<Bindings>,
    mut tanks: Query<(Entity, &mut TankCommand), With<Controlled>>,
    weapons: Query<(&Weapon, &WeaponIndex, &TankRoot), With<Muzzle>>,
) {
    let toggle = bindings.fuze_mode.just_pressed(&keys, &mouse);
    let step = match (
        bindings.round_next.just_pressed(&keys, &mouse),
        bindings.round_previous.just_pressed(&keys, &mouse),
    ) {
        (true, false) => Some(RoundStep::Next),
        (false, true) => Some(RoundStep::Previous),
        _ => None,
    };
    if step.is_none() && !toggle {
        return;
    }
    for (tank, mut command) in &mut tanks {
        let primary = weapons.iter().find_map(|(weapon, slot, root)| {
            (root.0 == tank && weapon.trigger == Trigger::Primary).then_some(slot.0)
        });
        // A vehicle without a primary trigger has no primary selection action.
        if let Some(slot) = primary {
            let weapon = u8::try_from(slot)
                .expect("the tank load gate limits weapon slots to the wire's u8");
            if let Some(step) = step {
                command.select_round = Some(RoundSelect { weapon, step });
            }
            if toggle {
                command.toggle_fuze = Some(weapon);
            }
        }
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

    /// The game's damage model: the input boundary refuses malformed levels, range and either
    /// aim frame before drive, servo or action consumers can observe them. The enum names the field.
    #[test]
    fn parsed_commands_refuse_each_malformed_input_field() {
        for (command, error) in [
            (
                TankCommand {
                    throttle: f32::NAN,
                    ..default()
                },
                CommandError::Throttle,
            ),
            (
                TankCommand {
                    throttle: 1.01,
                    ..default()
                },
                CommandError::Throttle,
            ),
            (
                TankCommand {
                    steer: f32::INFINITY,
                    ..default()
                },
                CommandError::Steer,
            ),
            (
                TankCommand {
                    steer: -1.01,
                    ..default()
                },
                CommandError::Steer,
            ),
            (
                TankCommand {
                    range: f32::NAN,
                    ..default()
                },
                CommandError::Range,
            ),
            (
                TankCommand {
                    range: -1.0,
                    ..default()
                },
                CommandError::Range,
            ),
        ] {
            assert_eq!(ParsedTankCommand::try_from(command).unwrap_err(), error);
        }
        for poison in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            *crate::aim::AIM_LIMIT * 1.01,
            -*crate::aim::AIM_LIMIT * 1.01,
        ] {
            for framed in [AimIntent::World, AimIntent::HullLocal] {
                let command = TankCommand {
                    aim: Some(framed(Vec3::splat(poison))),
                    ..default()
                };
                assert_eq!(
                    ParsedTankCommand::try_from(command).unwrap_err(),
                    CommandError::Aim
                );
            }
        }
        for level in [-1.0, 0.0, 1.0] {
            for framed in [AimIntent::World, AimIntent::HullLocal] {
                let point = Vec3::splat(*crate::aim::AIM_LIMIT);
                let command = TankCommand {
                    throttle: level,
                    steer: level,
                    aim: Some(framed(point)),
                    ..default()
                };
                let parsed = ParsedTankCommand::try_from(command).unwrap();
                assert_eq!(parsed.command(), command);
                assert_eq!(parsed.aim().unwrap().in_hull(&Affine3A::IDENTITY), point);
            }
        }
    }

    /// Every edge field is reported by [`TankCommand::has_edge`] and reset by
    /// [`TankCommand::clear_edges`] — the single-source-of-truth contract the edge set hangs off
    /// (`consume_edges` and the net input bridge both route through these two). A new edge added to
    /// one method but not the other fails this: `has_edge` would still report the field `clear_edges`
    /// left latched.
    ///
    /// `clear_edges` touches ONLY the edge fields — the levels and absolutes ride through untouched
    /// (the property `consume_edges` and the net input bridge both depend on). It leaves
    /// `fire_secondary` alone: that is a CONSUMABLE but not an EDGE, and folding it in here would
    /// kill sustained fire (`consume_edges` runs every tick). Only
    /// [`TankCommand::fail_consumables_closed`] clears both sets, and only on an unattested tick.
    #[test]
    fn clear_edges_resets_every_edge_has_edge_reports() {
        // Each latched-edge variant in isolation: has_edge true, then clear_edges makes it false.
        let edges: [fn(&mut TankCommand); 6] = [
            |c| c.fire_primary = true,
            |c| c.crew_swap = Some(CrewSwap::Start(CrewStation::Gunner, CrewStation::Loader)),
            |c| {
                c.select_round = Some(RoundSelect {
                    weapon: 0,
                    step: RoundStep::Next,
                })
            },
            |c| c.toggle_fuze = Some(0),
            |c| c.respawn = true,
            |c| c.repair = true,
        ];
        for set_edge in edges {
            let mut command = TankCommand::default();
            assert!(!command.has_edge(), "default command has no edge");
            set_edge(&mut command);
            assert!(command.has_edge(), "a latched edge is reported by has_edge");
            command.clear_edges();
            assert!(!command.has_edge(), "clear_edges resets the latched edge");
        }

        let mut command = TankCommand {
            throttle: 0.5,
            steer: -0.5,
            fire_secondary: true,
            aim: Some(AimIntent::World(Vec3::X)),
            range: 800.0,
            fire_primary: true,
            crew_swap: Some(CrewSwap::Cancel),
            select_round: Some(RoundSelect {
                weapon: 1,
                step: RoundStep::Previous,
            }),
            toggle_fuze: Some(0),
            respawn: true,
            repair: true,
            for_tick: 0,
        };
        command.clear_edges();
        assert_eq!(command.throttle, 0.5);
        assert_eq!(command.steer, -0.5);
        assert!(command.fire_secondary);
        assert_eq!(command.aim, Some(AimIntent::World(Vec3::X)));
        assert_eq!(command.range, 800.0);
        assert!(!command.has_edge(), "all edges cleared");
    }

    /// The game's damage model: a physical held repair key must author one consumable edge.
    /// Reading the key level instead of its press edge starts/cancels repeatedly and fails here.
    #[test]
    fn holding_repair_key_authors_only_one_edge() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let bindings = Bindings::default();
        let ButtonBinding::Key(key) = bindings.repair else {
            panic!("fixture uses a keyboard repair binding")
        };
        world.insert_resource(bindings);
        world.insert_resource(ButtonInput::<KeyCode>::default());
        world.insert_resource(ButtonInput::<MouseButton>::default());
        world.insert_resource(Ranging::default());
        world.insert_resource(Time::<Real>::default());
        world.insert_resource(DriveInput::default());
        let tank = world.spawn((Controlled, TankCommand::default())).id();
        world.resource_mut::<ButtonInput<KeyCode>>().press(key);
        world.run_system_once(gather_commands).unwrap();
        assert!(world.get::<TankCommand>(tank).unwrap().repair);
        world.run_system_once(consume_edges).unwrap();
        world.resource_mut::<ButtonInput<KeyCode>>().clear();
        world.run_system_once(gather_commands).unwrap();
        assert!(!world.get::<TankCommand>(tank).unwrap().repair);
        world.resource_mut::<ButtonInput<KeyCode>>().release(key);
        world.resource_mut::<ButtonInput<KeyCode>>().press(key);
        world.run_system_once(gather_commands).unwrap();
        assert!(world.get::<TankCommand>(tank).unwrap().repair);
    }

    /// Script the production gather with real ButtonInput edges and an explicit wall clock.
    /// Fixture durations are chosen relative to the shipped tap/hold boundary.
    struct DriveKeys {
        world: World,
        tank: Entity,
    }

    impl DriveKeys {
        fn new() -> Self {
            let mut world = World::new();
            world.insert_resource(Bindings::default());
            world.insert_resource(ButtonInput::<KeyCode>::default());
            world.insert_resource(ButtonInput::<MouseButton>::default());
            world.insert_resource(Ranging::default());
            world.insert_resource(Time::<Real>::default());
            world.insert_resource(DriveInput::default());
            let tank = world.spawn((Controlled, TankCommand::default())).id();
            Self { world, tank }
        }

        fn press(&mut self, key: KeyCode) {
            self.world.resource_mut::<ButtonInput<KeyCode>>().press(key);
        }

        fn release(&mut self, key: KeyCode) {
            self.world
                .resource_mut::<ButtonInput<KeyCode>>()
                .release(key);
        }

        fn gather(&mut self, dt: f64) -> crate::track::drive::DriveAxes {
            use bevy::ecs::system::RunSystemOnce;
            self.world
                .resource_mut::<Time<Real>>()
                .advance_by(std::time::Duration::from_secs_f64(dt));
            self.world.run_system_once(gather_commands).unwrap();
            self.world.resource_mut::<ButtonInput<KeyCode>>().clear();
            let c = self.world.get::<TankCommand>(self.tank).unwrap();
            crate::track::drive::DriveAxes {
                throttle: c.throttle,
                steer: c.steer,
            }
        }

        fn tap(&mut self, key: KeyCode) -> crate::track::drive::DriveAxes {
            self.press(key);
            self.gather(0.0);
            self.release(key);
            self.gather(crate::track::drive::drive_feel().tap_window_s.0 / 2.0)
        }
    }

    /// The Driver consumes a signed persistent lever, not a direction key. Stepping down from
    /// cruise must not request opposing intent during the tap window, on either axis.
    #[test]
    fn taps_step_both_signed_levers_without_flashing_opposing_intent() {
        let feel = crate::track::drive::drive_feel();
        // Fixture: two increments start at +50%; three decrements cross neutral to -25%.
        for (positive, negative, throttle) in [
            (KeyCode::KeyW, KeyCode::KeyS, true),
            (KeyCode::KeyD, KeyCode::KeyA, false),
        ] {
            let mut script = DriveKeys::new();
            let axis =
                |v: crate::track::drive::DriveAxes| if throttle { v.throttle } else { v.steer };
            let step = feel.notch_step.0;
            assert_eq!(axis(script.tap(positive)), step);
            let mut expected = (2.0 * step).min(1.0);
            assert_eq!(axis(script.tap(positive)), expected);
            for _ in 0..3 {
                expected = (expected - step).max(-1.0);
                script.press(negative);
                assert_eq!(axis(script.gather(0.0)), expected);
                assert_eq!(axis(script.gather(feel.tap_window_s.0 / 2.0)), expected);
                script.release(negative);
                assert_eq!(axis(script.gather(0.0)), expected);
                assert_eq!(
                    axis(script.gather(feel.tap_window_s.0)),
                    expected,
                    "notch persists"
                );
            }
        }
    }

    /// Repair changes the brake policy, not the player's retained throttle lever. The
    /// server must receive the same notch before, during and after the repair edge.
    #[test]
    fn repair_edge_preserves_the_throttle_notch() {
        use bevy::ecs::system::RunSystemOnce;
        let mut script = DriveKeys::new();
        let notch = script.tap(KeyCode::KeyW).throttle;
        script.press(KeyCode::KeyF);
        assert_eq!(script.gather(0.0).throttle, notch);
        assert!(script.world.get::<TankCommand>(script.tank).unwrap().repair);
        script.world.run_system_once(consume_edges).unwrap();
        script.release(KeyCode::KeyF);
        assert_eq!(script.gather(0.0).throttle, notch);
        assert!(!script.world.get::<TankCommand>(script.tank).unwrap().repair);
    }

    /// The game's damage model: a Driver station's HP state cannot take device ownership.
    /// The player can still edit the notch while the station is empty; staffing resumes that latest value.
    #[test]
    fn empty_driver_station_keeps_player_ownership_of_the_latest_notch() {
        use bevy::ecs::system::RunSystemOnce;
        let mut script = DriveKeys::new();
        script.world.insert_resource(DeviceOwner::Tank);
        let notch = script.tap(KeyCode::KeyW).throttle;
        let driver = script
            .world
            .spawn((
                crate::damage::CrewStation::Driver,
                crate::damage::VolumeOf(script.tank),
                crate::damage::Dead,
            ))
            .id();
        script.world.run_system_once(claim_device).unwrap();
        assert_eq!(*script.world.resource::<DeviceOwner>(), DeviceOwner::Tank);
        assert_eq!(script.gather(0.0).throttle, notch);
        let updated = script.tap(KeyCode::KeyW).throttle;
        assert!(updated > notch);
        script
            .world
            .entity_mut(driver)
            .remove::<crate::damage::Dead>();
        script.world.run_system_once(claim_device).unwrap();
        assert_eq!(*script.world.resource::<DeviceOwner>(), DeviceOwner::Tank);
        assert_eq!(script.gather(0.0).throttle, updated);
    }

    /// A hold owns the full axis after the window, then releases to coast/straight. Keeping the
    /// previous notch here would leave the player's vehicle cruising or turning after release.
    #[test]
    fn a_hold_overrides_and_release_clears_each_notch() {
        let step = crate::track::drive::drive_feel().notch_step.0;
        let window = crate::track::drive::drive_feel().tap_window_s.0;
        for key in [KeyCode::KeyW, KeyCode::KeyS, KeyCode::KeyA, KeyCode::KeyD] {
            let mut script = DriveKeys::new();
            script.tap(key);
            script.press(key);
            let start = script.gather(0.0);
            assert!(
                start.throttle.abs() == (2.0 * step).min(1.0)
                    || start.steer.abs() == (2.0 * step).min(1.0)
            );
            let held = script.gather(window);
            assert_eq!(
                script.world.resource::<DriveInput>().notches(),
                held,
                "the HUD must show the effective held output"
            );
            assert!(held.throttle.abs() == 1.0 || held.steer.abs() == 1.0);
            script.release(key);
            assert_eq!(
                script.gather(0.0),
                crate::track::drive::DriveAxes::default()
            );
            let next = script.tap(key);
            assert!(next.throttle.abs() == step || next.steer.abs() == step);
        }
    }

    /// The keyboard feel contract includes the window and the pedal rise. A mutant that sends
    /// full intent immediately, or waits for release to apply a notch, misses this derived time.
    #[test]
    fn a_hold_reaches_full_at_the_window_plus_remaining_pedal_travel() {
        use crate::track::drive::{DriveAxes, drive_feel, shape_drive};
        let feel = drive_feel();
        let expected_s =
            feel.tap_window_s.0 + f64::from((1.0 - feel.notch_step.0) / feel.rise_per_second.0);
        let dt = 1.0 / 256.0; // Fixture: binary-exact sampling bounds the derived full time to one sample.
        let mut script = DriveKeys::new();
        script.press(KeyCode::KeyW);
        let mut target = script.gather(0.0);
        let mut axes = DriveAxes::default();
        let mut elapsed = 0.0;
        while axes.throttle < 1.0 {
            assert!(
                elapsed < expected_s + dt,
                "hold must finish within its derived bound"
            );
            target = if elapsed == 0.0 {
                target
            } else {
                script.gather(dt)
            };
            axes = shape_drive(axes, target, dt as f32);
            elapsed += dt;
        }
        assert!(
            (elapsed - expected_s).abs() <= dt,
            "{elapsed} vs derived {expected_s}"
        );
        println!(
            "MEASURED scripted hold full at {elapsed:.8} s; DERIVED continuous time {expected_s:.8} s"
        );
    }

    /// The Driver must get neutral on the first opposing hold tick, rather than another tick
    /// asking for forward motion. This covers the production gather-to-shaper boundary.
    #[test]
    fn a_reversal_hold_snaps_on_its_first_tick_after_the_tap_window() {
        use crate::track::drive::{DriveAxes, drive_feel, shape_drive};
        let mut script = DriveKeys::new();
        script.tap(KeyCode::KeyW);
        script.tap(KeyCode::KeyW);
        script.press(KeyCode::KeyS);
        let target = script.gather(0.0);
        let axes = DriveAxes {
            throttle: 0.5,
            steer: 0.0,
        }; // Fixture: forward cruise.
        assert_eq!(
            target.throttle,
            (2.0 * drive_feel().notch_step.0).min(1.0) - drive_feel().notch_step.0
        );
        let held = script.gather(drive_feel().tap_window_s.0);
        assert_eq!(held.throttle, -1.0);
        assert_eq!(shape_drive(axes, held, 1.0 / 64.0).throttle, 0.0); // Fixture tick.
    }

    /// Real devices may deliver both edges in one frame. That tap still counts once, and the
    /// bounded wire axis must survive repeated taps, a chord, and a change of controlled body.
    #[test]
    fn subframe_taps_clamp_and_opposite_keys_or_possession_cancel_the_lever() {
        let step = crate::track::drive::drive_feel().notch_step.0;
        let mut script = DriveKeys::new();
        script.press(KeyCode::KeyW);
        script.release(KeyCode::KeyW);
        assert_eq!(script.gather(0.0).throttle, step);
        assert_eq!(script.gather(0.0).throttle, step, "tap counts once");
        for _ in 0..((1.0 / step).ceil() as usize + 1) {
            script.tap(KeyCode::KeyW);
        } // Fixture: enough to exceed the bound.
        assert_eq!(script.gather(0.0).throttle, 1.0);
        script.press(KeyCode::KeyW);
        script.press(KeyCode::KeyS);
        assert_eq!(script.gather(0.0).throttle, 0.0);
        script.release(KeyCode::KeyW);
        script.release(KeyCode::KeyS);
        script.gather(0.0);
        script.tap(KeyCode::KeyW);
        script.world.entity_mut(script.tank).remove::<Controlled>();
        script.tank = script
            .world
            .spawn((Controlled, TankCommand::default()))
            .id();
        assert_eq!(
            script.gather(0.0).throttle,
            0.0,
            "new body has no inherited cruise"
        );
    }

    /// One intention, two frames, one point: [`AimIntent`]'s conversions are the seam every
    /// consumer of the wire goes through, so they must agree exactly with the hull pose they are
    /// handed — under a hull that is ROTATED AND TRANSLATED, where a `transform_vector3` standing in
    /// for a `transform_point3` (or the reverse) silently drops or adds the whole translation.
    #[test]
    fn an_aim_intent_names_the_same_point_in_either_frame() {
        // A hull well away from the origin and off every axis, so no term can cancel by luck.
        let hull = Affine3A::from_rotation_translation(
            Quat::from_euler(EulerRot::YXZ, 0.7, -0.2, 0.13),
            Vec3::new(137.0, 4.0, -512.0),
        );
        let to_local = hull.inverse();
        let local = Vec3::new(61.0, -3.0, -498.0);
        let world = hull.transform_point3(local);

        assert!(
            AimIntent::World(world).in_hull(&to_local).distance(local) < 1e-2,
            "a world place named in the hull frame is the bearing it stands on",
        );
        assert!(
            AimIntent::HullLocal(local).in_world(&hull).distance(world) < 1e-2,
            "a hull bearing named in the world is the place it points at",
        );
        // The identity arms: an intention already in the frame asked for is handed back untouched,
        // whatever pose comes with it.
        assert_eq!(AimIntent::World(world).in_world(&hull), world);
        assert_eq!(AimIntent::HullLocal(local).in_hull(&to_local), local);
        assert_eq!(AimIntent::World(world).point(), world);
        assert_eq!(AimIntent::HullLocal(local).point(), local);
    }

    /// Every system name registered in `RunFixedMainLoop`, read off the schedule GRAPH so no
    /// initialization or `update()` is needed (`Schedule::systems` requires a run first; the graph
    /// holds the nodes from `add_systems` onward — vendored bevy_ecs-0.19.0 `ScheduleGraph`).
    fn run_fixed_main_loop_systems(app: &App) -> Vec<String> {
        let schedules = app.world().resource::<bevy::ecs::schedule::Schedules>();
        schedules
            .get(RunFixedMainLoop)
            .map(|schedule| {
                schedule
                    .graph()
                    .systems
                    .iter()
                    .map(|(_, system, _)| system.name().to_string())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// SECURITY BOUNDARY (the reason [`offline_auto_fire_plugin`] is a separate mount): the shared
    /// device-gather plugin — the one `NetClientPlugin` composes into every packaged network client
    /// — must not register the scripted trigger, *even in an app armed for it* ([`AutoFireArmed`],
    /// which stands in for `SPIKE_AUTO_FIRE`). A release client compiles the hook, so if it rode
    /// along in `client_plugin` a player could arm it with an env var and have scripted trigger
    /// edges sent to a live server as genuine input.
    ///
    /// The second half is the control: the same probe DOES see `auto_fire` once the offline mount is
    /// added, so the first assertion is proof of absence rather than proof of a blind probe.
    #[test]
    fn the_shared_device_gather_never_arms_auto_fire_even_when_armed() {
        let mut shared = App::new();
        shared.insert_resource(AutoFireArmed(true));
        shared.add_plugins(client_plugin);
        let names = run_fixed_main_loop_systems(&shared);
        assert!(
            names.iter().any(|name| name.contains("gather_commands")),
            "the probe must see the shared mount's own system: {names:?}",
        );
        assert!(
            !names.iter().any(|name| name.contains("auto_fire")),
            "the shared client mount (which NetClientPlugin composes) armed the scripted trigger \
             — a packaged client could then drive a live server: {names:?}",
        );

        let mut offline = App::new();
        offline.insert_resource(AutoFireArmed(true));
        offline.add_plugins((client_plugin, offline_auto_fire_plugin));
        assert!(
            run_fixed_main_loop_systems(&offline)
                .iter()
                .any(|name| name.contains("auto_fire")),
            "control: the offline-only mount must arm the trigger the assertion above denies",
        );
    }

    /// The other half of the boundary, and the half a runtime probe cannot state: `run_offline` is
    /// the crate's ONLY mount site. The check above proves one shared plugin is clean; this proves
    /// that no OTHER plugin mounts the hook either — including every plugin `NetClientPlugin`
    /// composes — by scanning each source file for the mount identifier.
    ///
    /// Source-scan enforcement follows `render_policy`'s
    /// `no_raw_render_layers_outside_this_module`: the module that owns a dangerous capability
    /// pins its call sites in text, because "nobody else calls this" is not expressible in the
    /// type system.
    #[test]
    fn offline_auto_fire_plugin_is_mounted_only_inside_run_offline() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut sites = Vec::new();
        let mut stack = vec![src.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).expect("src/ is readable") {
                let path = entry.expect("a readable directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|ext| ext != "rs") {
                    continue;
                }
                let relative = path
                    .strip_prefix(src.parent().expect("src/ has a parent"))
                    .expect("every scanned path is under the manifest")
                    .to_string_lossy()
                    .replace('\\', "/");
                // `command.rs` defines the plugin and documents it; every OTHER file mentioning the
                // identifier outside a comment is mounting it.
                if relative == "src/command.rs" {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("a readable source file");
                for (line, text) in source.lines().enumerate() {
                    if text.contains("offline_auto_fire_plugin")
                        && !text.trim_start().starts_with("//")
                    {
                        sites.push((relative.clone(), line + 1, text.trim().to_string()));
                    }
                }
            }
        }

        // The one legal site, located by name: the body of `run_offline`, from its signature to the
        // next item at column 0.
        let lib = std::fs::read_to_string(src.join("lib.rs")).expect("src/lib.rs is readable");
        let lines: Vec<&str> = lib.lines().collect();
        let start = lines
            .iter()
            .position(|line| line.starts_with("pub fn run_offline()"))
            .expect("run_offline is declared at column 0 in lib.rs");
        let end = start
            + 1
            + lines[start + 1..]
                .iter()
                .position(|line| line.starts_with('}'))
                .expect("run_offline's body is closed at column 0");

        assert_eq!(
            sites.len(),
            1,
            "the scripted trigger must have exactly ONE mount site in the crate (inside \
             `run_offline`, the netcode-free offline root) — every extra site is a path by which a \
             shipped network client could arm it:\n  {}",
            sites
                .iter()
                .map(|(file, line, text)| format!("{file}:{line}: {text}"))
                .collect::<Vec<_>>()
                .join("\n  "),
        );
        let (file, line, text) = &sites[0];
        assert!(
            file == "src/lib.rs" && (start..=end).contains(&(line - 1)),
            "the only mount site must be inside `run_offline` (lib.rs lines {}-{}), not \
             {file}:{line}: {text}",
            start + 1,
            end + 1,
        );
    }

    /// The controlled tank's command as each fixed tick's sim reads it.
    #[derive(Resource, Default)]
    struct TickReads(Vec<TankCommand>);

    /// Latch the knockout inside the next tick, after that tick's command was read.
    #[derive(Resource, Default)]
    struct KnockOutNextTick(bool);

    fn read_tick(own: Query<&TankCommand, With<Controlled>>, mut reads: ResMut<TickReads>) {
        reads.0.extend(own.iter().copied());
    }

    fn latch_knockout(
        mut arm: ResMut<KnockOutNextTick>,
        own: Query<Entity, With<Controlled>>,
        mut commands: Commands,
    ) {
        if std::mem::take(&mut arm.0) {
            for tank in &own {
                commands.entity(tank).insert(TankKnockedOut {
                    reason: crate::damage::KnockoutReason::CrewLoss,
                });
            }
        }
    }

    /// Offline the sim reads [`TankCommand`] itself: a knockout latched inside the first tick of a
    /// frame of several leaves the command neutral for the frame's later ticks, and the device with
    /// the death flow by the end of the frame.
    #[test]
    fn a_knockout_latched_in_a_tick_neutralizes_the_frames_later_ticks() {
        const TICKS_PER_FRAME: u32 = 3;
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
                Time::<Fixed>::default().timestep() * TICKS_PER_FRAME,
            ))
            .add_plugins((core_plugin, client_plugin))
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .init_resource::<Ranging>()
            .init_resource::<TickReads>()
            .init_resource::<KnockOutNextTick>()
            .add_systems(FixedUpdate, (read_tick, latch_knockout).chain());
        app.world_mut().spawn((TankCommand::default(), Controlled));
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyW);
        for _ in 0..2 {
            app.update();
        }
        let alive = std::mem::take(&mut app.world_mut().resource_mut::<TickReads>().0);
        assert!(
            !alive.is_empty()
                && alive
                    .iter()
                    .all(|command| command.throttle
                        == crate::track::drive::drive_feel().notch_step.0),
            "alive, every tick reads the gathered command: {alive:?}"
        );

        app.world_mut().resource_mut::<KnockOutNextTick>().0 = true;
        app.update();
        let reads = &app.world().resource::<TickReads>().0;
        assert_eq!(reads.len(), TICKS_PER_FRAME as usize, "{reads:?}");
        assert_eq!(
            reads[0].throttle,
            crate::track::drive::drive_feel().notch_step.0,
            "the latching tick read its command before the knockout: {reads:?}"
        );
        assert!(
            reads[1..]
                .iter()
                .all(|command| *command == TankCommand::default()),
            "the frame's later ticks read the neutral command: {reads:?}"
        );
        assert_eq!(
            *app.world().resource::<DeviceOwner>(),
            DeviceOwner::DeathFlow
        );
        assert_eq!(
            app.world().resource::<DriveInput>().notches(),
            crate::track::drive::DriveAxes::default()
        );
    }
}
