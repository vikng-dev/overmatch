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

/// The scripted-trigger hook, as its OWN mount — the offline capture composition (`run_offline`)
/// is the only caller, and that is a security boundary, not a tidiness preference.
///
/// [`client_plugin`] above is shared: `NetClientPlugin` mounts it too, and `dev_tools` is a DEFAULT
/// feature, so a packaged release client builds `auto_fire`'s code. Had the registration stayed in
/// the shared mount, any player could launch the shipped client with `SPIKE_AUTO_FIRE=1` and have
/// scripted trigger edges written into `TankCommand` — which the net input bridge then sends to a
/// LIVE server as genuine player input. Registering it here instead makes the hook STRUCTURALLY
/// absent from every network-client app: there is no env var, no config and no build flag that can
/// reach it, because the systems are never added to that `App` at all.
///
/// Two tests pin the boundary in both directions: the shared mount never arms the trigger even with
/// the env var set (`the_shared_device_gather_never_arms_auto_fire_even_when_the_env_var_is_set`),
/// and `run_offline` is the crate's ONLY mount site
/// (`offline_auto_fire_plugin_is_mounted_only_inside_run_offline`, a source scan — "no other plugin
/// mounts this" is not expressible in the type system).
#[cfg(feature = "dev_tools")]
pub fn offline_auto_fire_plugin(app: &mut App) {
    if !crate::env_flag("SPIKE_AUTO_FIRE", false) {
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
            .in_set(GameplaySet),
    );
}

/// The hardcoded auto-fire timeline, in `Time<Real>` seconds — the SAME clock `frame_cost` stamps
/// its rows with, so a capture can be split into windows by these boundaries alone. Stated as one
/// string because it is logged verbatim at arm time: the capture's own record of what it drove.
#[cfg(feature = "dev_tools")]
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
#[cfg(feature = "dev_tools")]
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

    /// Serializes the tests that write `SPIKE_AUTO_FIRE` — the variable is process-global, so two
    /// of these at once would each see the other's value (the same discipline `settings::store`'s
    /// `ENV_LEASE` applies to `OVERMATCH_CONFIG_DIR`).
    #[cfg(feature = "dev_tools")]
    static AUTO_FIRE_ENV_LEASE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// `SPIKE_AUTO_FIRE=1` for the body's lifetime, restored on drop.
    #[cfg(feature = "dev_tools")]
    struct ArmedEnv {
        previous: Option<String>,
        _lease: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(feature = "dev_tools")]
    impl ArmedEnv {
        fn new() -> Self {
            let lease = AUTO_FIRE_ENV_LEASE
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let previous = std::env::var("SPIKE_AUTO_FIRE").ok();
            // SAFETY: the lease makes this body single-threaded with respect to the variable, and
            // `Drop` restores it.
            unsafe { std::env::set_var("SPIKE_AUTO_FIRE", "1") };
            Self {
                previous,
                _lease: lease,
            }
        }
    }

    #[cfg(feature = "dev_tools")]
    impl Drop for ArmedEnv {
        fn drop(&mut self) {
            // SAFETY: as above.
            unsafe {
                match self.previous.take() {
                    Some(value) => std::env::set_var("SPIKE_AUTO_FIRE", value),
                    None => std::env::remove_var("SPIKE_AUTO_FIRE"),
                }
            }
        }
    }

    /// Every system name registered in `RunFixedMainLoop`, read off the schedule GRAPH so no
    /// initialization or `update()` is needed (`Schedule::systems` requires a run first; the graph
    /// holds the nodes from `add_systems` onward — vendored bevy_ecs-0.19.0 `ScheduleGraph`).
    #[cfg(feature = "dev_tools")]
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
    /// — must not register the scripted trigger, *even with `SPIKE_AUTO_FIRE` set*. A release client
    /// builds `dev_tools` (a default feature), so if the hook rode along in `client_plugin` a player
    /// could arm it with an env var and have scripted trigger edges sent to a live server as
    /// genuine input.
    ///
    /// The second half is the control: the same probe DOES see `auto_fire` once the offline mount is
    /// added, so the first assertion is proof of absence rather than proof of a blind probe.
    #[test]
    #[cfg(feature = "dev_tools")]
    fn the_shared_device_gather_never_arms_auto_fire_even_when_the_env_var_is_set() {
        let _armed = ArmedEnv::new();

        let mut shared = App::new();
        shared.add_plugins(client_plugin);
        let names = run_fixed_main_loop_systems(&shared);
        assert!(
            names.iter().any(|name| name.contains("gather_commands")),
            "the probe must see the shared mount's own system: {names:?}",
        );
        assert!(
            !names.iter().any(|name| name.contains("auto_fire")),
            "the shared client mount (which NetClientPlugin composes) armed the scripted trigger \
             from an env var — a packaged client could then drive a live server: {names:?}",
        );

        let mut offline = App::new();
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
}
