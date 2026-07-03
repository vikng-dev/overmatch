# lightyear 0.28 integration map (Overmatch spike)

Research date: 2026-07-03. lightyear 0.28.0 released 2026-06-26 (PR #1361 replaced the old
replication core with `bevy_replicon` 0.41). All findings below were verified by cloning the
actual crate source (tag `0.28.0`, commit `28e823d`) into a scratch dir and grepping it, plus
`cargo add --dry-run` against the published crates.io package, plus a matching clone of
`bevy_replicon` `v0.41.0`. The lightyear book (`cbournhonesque.github.io/lightyear/book`) was
checked too, but **parts of it are stale** relative to 0.28 source — flagged inline where found.

Source roots used (all under
`/private/tmp/claude-502/.../scratchpad/lightyear-research/`):
- `lightyear-src/` — clone of `cBournhonesque/lightyear` @ tag `0.28.0`
- `replicon-src/` — clone of `projectharmonia/bevy_replicon` @ tag `v0.41.0`
- `dryrun_probe/` — scratch crate used for `cargo add --dry-run` feature-flag verification

Grounding read from the Overmatch repo: `src/lib.rs` (`SimPlugin`/`ClientPlugin` split),
`src/command.rs` (`TankCommand`), `src/tank.rs` (rig binder, `TankRoot(Entity)`),
`src/headless_test.rs` (proven headless-server recipe).

---

## 1. Dependencies

lightyear ships as **one meta-crate wrapping ~25 subcrates** selected by feature flags — you do
not depend on subcrates individually for a normal integration (the meta-crate re-exports their
public APIs under `lightyear::prelude::*` / `lightyear::prelude::{client,server}::*`). You only
add a subcrate directly if you need something the meta-crate doesn't re-export (we didn't hit
that case).

Verified via `cargo add lightyear@0.28 --features "server,client,netcode,udp,avian3d,input_native" --dry-run`
against the real crates.io index (source: `dryrun_probe/`, live 2026-07-03):

```
Features as of v0.28.0:
+ avian3d, client, input_native, interpolation, netcode, prediction, replication, server, std, udp
(default already includes: client, interpolation, prediction, replication, server, std)
```
source: `dryrun_probe` cargo-add dry run; cross-checked against
`lightyear-src/crates/core/lightyear/Cargo.toml` `[features] default = [std, client, server, replication, prediction, interpolation]`

A single binary that can run as **either** dedicated server or client (our current single-binary
pattern, matching `SimPlugin`/`ClientPlugin`) needs both `client` and `server` features — both
are default-on, so plain `lightyear = "0.28"` already has what you need; you mainly add the
transport/input/physics feature flags explicitly.

Proposed Cargo.toml additions (not applied — spike only):

```toml
[dependencies]
# ... existing deps unchanged ...
lightyear = { version = "0.28", default-features = false, features = [
    "std",
    "client", "server",         # one binary, both roles (matches SimPlugin/ClientPlugin split)
    "replication", "prediction", "interpolation",
    "input_native",             # plain-struct input, NOT leafwing — see §4
    "netcode", "udp",           # dev/LAN transport — see §2
    "avian3d",                  # pulls in lightyear_avian3d re-exports under lightyear::avian3d::*
] }
lightyear_avian3d = { version = "0.28", default-features = false, features = ["3d", "f32"] }
```

Notes:
- `lightyear_avian3d` is **not** auto-added by `lightyear`'s `avian3d` feature as a usable plugin
  — the feature only makes `lightyear::avian3d::*` types resolve; `LightyearAvianPlugin` still
  has to be added manually by the app (its own doc comment says so explicitly). Depending on it
  directly (as above) or just enabling `lightyear`'s `avian3d` feature both work; the workspace's
  own examples do the latter (`lightyear::avian3d::plugin::LightyearAvianPlugin`).
  source: `lightyear-src/crates/integration/avian/src/plugin.rs:119-121` (doc comment: "this
  plugin is NOT added automatically by ClientPlugins/ServerPlugins, you have to add it manually!")
- `avian3d.workspace = true` in lightyear's own Cargo.toml pins to `avian3d = { version = "0.7",
  default-features = false }` — **exact match** to our repo's `avian3d = "0.7"`.
  source: `lightyear-src/Cargo.toml:245`, our `Cargo.toml:10`
- `lightyear_avian3d`'s own Cargo.toml default features: `["std", "3d", "avian3d/parry-f32"]` —
  we'd want `f32` explicitly too (avian3d defaults to f32 anyway on most platforms, but lightyear's
  docs.rs metadata pins `features = ["lightyear_avian3d/f32"]` explicitly, so best to be explicit).
  source: `lightyear-src/crates/integration/avian3d/Cargo.toml:22-24`, `crates/core/lightyear/Cargo.toml`
  `[package.metadata.docs.rs] features = ["lightyear_avian3d/f32", ...]`
- Do NOT add `leafwing` or `input_bei` features — see §4, we want `input_native` only.
- `serde`, `glam` (with `serde` feature, already on in our Cargo.toml), `ron` — all already
  present in our Cargo.toml and compatible; no changes needed there.
- No WASM target for a dedicated tank-duel server/client, so `webtransport`/`websocket` are
  skippable; `udp` + `netcode` cover the LAN/dev spike. Real internet play later would likely add
  `webtransport` for NAT traversal, but that's out of scope for the spike.

---

## 2. Server app setup

**Tick ownership**: lightyear does not run a parallel clock — it directly drives Bevy's own
`Time<Fixed>` resource. `TimelinePlugin::build` (part of `SharedPlugins`, mounted by both
`ClientPlugins` and `ServerPlugins`) does:

```rust
app.world_mut().resource_mut::<Time<Fixed>>().set_timestep(self.tick_duration);
```

and also fires a `SetTickDuration` event on `finish()` that an observer applies to `Time<Fixed>`
again. So **`Time<Fixed>` IS lightyear's tick** — there is no separate "lightyear tick" you have
to reconcile against Bevy's fixed timestep. You pass the tick duration once, at plugin-group
construction time (`ServerPlugins { tick_duration }` / `ClientPlugins { tick_duration }`), and
everything (physics `FixedUpdate`, our `command::core_plugin`'s `FixedUpdate` systems, replication
send/receive) rides the same `Time<Fixed>`.
source: `lightyear-src/crates/core/core/src/timeline.rs:164-189` (`TimelinePlugin`)

Our target: `ServerPlugins { tick_duration: Duration::from_secs_f64(1.0/64.0) }` to match our
existing `Time<Fixed>` default of 64 Hz (`src/lib.rs` doc comment: "fixed clock 64 Hz default
`Time<Fixed>`"). The examples harness uses exactly this pattern with its own
`FIXED_TIMESTEP_HZ: f64 = 64.0` constant — i.e. **64 Hz is literally the value the official
examples use**, not just a coincidence with our number.
source: `lightyear-src/examples/common/src/shared.rs:4` (`pub const FIXED_TIMESTEP_HZ: f64 = 64.0;`)

**Minimal dedicated-server plugin set** (headless, UDP+netcode transport), assembled from the
`lightyear_examples_common` harness (`ExampleServer`/`cli.rs`) which is the closest thing to a
"reference minimal server" in the repo:

```rust
use bevy::prelude::*;
use bevy::app::ScheduleRunnerPlugin;
use core::net::{Ipv4Addr, SocketAddr};
use core::time::Duration;
use lightyear::prelude::*;
use lightyear::prelude::server::*;
use lightyear::netcode::{NetcodeServer, NetcodeConfig};

fn main() {
    let mut app = App::new();
    // MinimalPlugins: no window, no winit, no GPU. Unthrottled loop is fine for a dedicated
    // server (examples harness only throttles *client* loops to emulate frame pacing).
    app.add_plugins((MinimalPlugins, TransformPlugin, bevy::state::app::StatesPlugin));

    app.add_plugins(lightyear::prelude::server::ServerPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });

    // Protocol registration must happen AFTER ServerPlugins, BEFORE spawning the Server entity.
    app.add_plugins(ProtocolPlugin); // registers TankCommand input + replicated components

    // Netcode + UDP transport, bound to a LAN port, with a zeroed dev private key
    // (Authentication::Manual on the client side matches this).
    let server_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 5888);
    let server = app.world_mut().spawn((
        Name::new("Server"),
        NetcodeServer::new(NetcodeConfig {
            protocol_id: 0,
            private_key: [0; 32], // DEV ONLY — see auth note below
            ..default()
        }),
        LocalAddr(server_addr),
        ServerUdpIo::default(),
    )).id();
    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(Start { entity: server });
    });

    // Mount SimPlugin (our existing authority layer) here, after protocol registration.
    app.add_plugins(overmatch::SimPlugin);

    app.run();
}
```
source (pattern assembled from): `lightyear-src/examples/common/src/server.rs` (`ExampleServer::on_add`,
lines 99-171 — the `Udp` transport arm), `lightyear-src/examples/common/src/cli.rs:382-397`
(`new_headless_app`, confirms `MinimalPlugins` + `TransformPlugin` + `StatesPlugin` is the
examples' own headless recipe — very close to our proven `headless_test.rs` recipe, though ours
uses full `DefaultPlugins` with `backends: None` instead of `MinimalPlugins`)

**UNCERTAIN**: our `headless_test.rs` proves `DefaultPlugins` + `RenderPlugin{backends:None}` +
no window is required for *our* asset/gltf-scene loading pipeline (an earlier hand-rolled
`MinimalPlugins` assembly didn't complete gltf loads — see the comment at the top of that file).
The lightyear examples harness's `new_headless_app` uses plain `MinimalPlugins` because those
examples spawn simple primitive colliders, not glb scenes. **For our actual tank server (which
must load the same `.glb` + `.tank.ron` spec the client does, to build authoritative colliders),
we likely need our proven `DefaultPlugins` + `backends: None` recipe, not bare `MinimalPlugins`.**
This needs to be validated in the spike itself — it's the single highest-risk assumption in this
whole map, since it combines two things (lightyear's tick ownership + our asset-loading headless
recipe) that no example in the lightyear repo combines.

**Dev-mode auth (skip real connect-token infrastructure)**: `Authentication::Manual` is the
documented, source-commented "testing purposes only" bypass — no token server needed. The client
supplies `server_addr`, `client_id`, `private_key`, `protocol_id` directly; matching
`private_key: [0; 32]` on both ends (zeroed key, as the examples harness does via
`SHARED_SETTINGS`) is sufficient for LAN/dev.

```rust
pub enum Authentication {
    Token(ConnectToken),   // production: token from a backend
    Manual { server_addr: SocketAddr, client_id: u64, private_key: Key, protocol_id: u64 }, // dev
    None,                  // default; can't connect yet
}
```
source: `lightyear-src/crates/connection/netcode/src/auth.rs:24-47` — doc comment on `Manual`:
"This is only useful for testing purposes. In production, the client should not have access to
the `private_key`."

The examples harness's shared zeroed key (`SHARED_SETTINGS.private_key = [0; 32]`,
`protocol_id: 0`) is exactly this dev pattern, reused verbatim across every example.
source: `lightyear-src/examples/common/src/shared.rs:9-15`

---

## 3. Client app setup

Minimal connect-by-IP client, assembled from `lightyear_examples_common::client::ExampleClient`:

```rust
use bevy::prelude::*;
use core::net::{Ipv4Addr, SocketAddr};
use core::time::Duration;
use lightyear::prelude::*;
use lightyear::prelude::client::*;
use lightyear::netcode::{NetcodeClient, client_plugin::NetcodeConfig};
use lightyear::netcode::Authentication;

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins); // real client: window, renderer, etc.

    app.add_plugins(lightyear::prelude::client::ClientPlugins {
        tick_duration: Duration::from_secs_f64(1.0 / 64.0),
    });
    app.add_plugins(ProtocolPlugin); // SAME protocol registration as the server, verbatim

    let server_addr: SocketAddr = "127.0.0.1:5888".parse().unwrap();
    let client_addr = SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0); // OS picks port
    let client = app.world_mut().spawn((
        Client::default(),
        Link::new(None), // no conditioner for a real LAN test
        LocalAddr(client_addr),
        PeerAddr(server_addr),
        PredictionManager::default(),
        NetcodeClient::new(
            Authentication::Manual {
                server_addr, client_id: 1, private_key: [0; 32], protocol_id: 0,
            },
            NetcodeConfig { client_timeout_secs: 3, token_expire_secs: -1, ..default() },
        ).unwrap(),
        UdpIo::default(),
    )).id();
    app.add_systems(Startup, move |mut commands: Commands| {
        commands.trigger(Connect { entity: client });
    });

    app.add_plugins(overmatch::SimPlugin); // client-side prediction re-simulates our sim locally
    app.add_plugins(overmatch::ClientPlugin); // device gather, presentation
    app.run();
}
```
source: `lightyear-src/examples/common/src/client.rs:44-126` (`ExampleClient::on_add`, the `Udp`
transport arm) and `:128-132` (`connect` triggers the `Connect` event)

Order matters: `ClientPlugins` (or `ServerPlugins`) must be added **before** `ProtocolPlugin`
(component/input registration), which must happen **before** spawning the `Client`/`Server`
entity, per the doc comment on `ServerPlugins`: "first add the `ServerPlugins`, then build your
protocol ..., then spawn your `Server` entity." The same ordering rule holds for the client side.
source: `lightyear-src/crates/core/lightyear/src/server.rs:18-24`

---

## 4. Input path — `TankCommand` on lightyear 0.28's input system

**Three input plugin variants exist**: `lightyear_inputs_native` (plain struct, self-written
gather — **this is the fit for `TankCommand`**), `lightyear_inputs_leafwing` (leafwing-input-manager
action maps), `lightyear_inputs_bei` (bevy_enhanced_input). All three share the same underlying
`lightyear_inputs` crate machinery (`InputBuffer`, `ActionStateSequence`, tick-synced messages);
they only differ in how the per-tick "action state" struct is produced. Confirmed by workspace
members list: `crates/inputs/{inputs, inputs_native, inputs_leafwing, input_bei}`.
source: `lightyear-src/Cargo.toml:17-24` (workspace members), `lightyear-src/crates/core/lightyear/Cargo.toml`
`[features] input_native = ["dep:lightyear_inputs", "dep:lightyear_inputs_native"]`

**`input_native` is exactly the "plain struct, we gather it ourselves" case** — `simple_box`, the
canonical minimal example, uses it with a hand-rolled `Direction`/`Inputs` enum and its own
`buffer_input` system in `FixedPreUpdate`, no action-map DSL at all:

```rust
// protocol.rs
app.add_plugins(input::native::InputPlugin::<Inputs>::default());

// client.rs — FixedPreUpdate, in_set(InputSystems::WriteClientInputs)
fn buffer_input(
    timeline: Res<LocalTimeline>,
    mut query: Query<&mut ActionState<Inputs>, With<InputMarker<Inputs>>>,
    keypress: Option<Res<ButtonInput<KeyCode>>>,
) {
    if let Ok(mut action_state) = query.single_mut() {
        // ... read devices, write action_state.0 directly ...
    }
}
```
source: `lightyear-src/examples/simple_box/src/protocol.rs:120`,
`lightyear-src/examples/simple_box/src/client.rs:49-89`

**The `A` type requirements** (what `TankCommand` needs to satisfy): `Serialize + DeserializeOwned
+ Clone + PartialEq + Send + Sync + Debug + Default + MapEntities + Reflectable + FromReflect`.
Our `TankCommand` already derives `Serialize`/`Deserialize`/`Clone`/`Copy` and is `Default`; we'd
additionally need `PartialEq`, `Debug`, `Reflect`/`FromReflect`, and a `MapEntities` impl (trivial
— `TankCommand.aim: Option<Vec3>` has no `Entity` fields, so the impl is a no-op body, exactly
like the `NativeInput`/`Inputs::map_entities` no-ops in the examples).
source: `lightyear-src/crates/inputs/inputs_native/src/plugin.rs:19-32` (trait bounds on
`InputPlugin<A>`); `lightyear-src/crates/tests/src/protocol.rs:162-164` (no-op `MapEntities` for
a plain `NativeInput(i16)`)

**Registration**: `app.add_plugins(input::native::InputPlugin::<TankCommand>::default())` in the
shared `ProtocolPlugin`, mounted on both client and server (feature-gates its own client-vs-server
internals — `ClientInputPlugin`/`ServerInputPlugin` — via `#[cfg(feature = "client"/"server")]`,
so one call in shared code is correct for our single-binary-both-roles setup).
source: `lightyear-src/crates/inputs/inputs_native/src/plugin.rs:32-56`

**The buffer + tick sync mechanism**: `ActionState<A>` is a whole-state snapshot per tick (not a
diff/delta stream) — `Compressed::Input(value)` per tick, with `Compressed::SameAsPrecedent` used
for redundancy compression across the wire when consecutive ticks are identical, and
`Compressed::Absent` distinguishing "no input received" from "input was the default." This matters
for us: **`A::Default` must mean "no input this tick,"** which is already true of
`TankCommand::default()` (throttle/steer = 0, both fire flags false, aim = None).
source: `lightyear-src/crates/inputs/inputs_native/src/action_state.rs:14-21` (doc comment: "It is
important to distinguish between 'no input' ... and 'input not received' ...")

**"Server reads client X's input for tick N"** — literally `input_buffer.get_predict(tick)` on
the entity's `InputBuffer<S::Snapshot, S::Action>` component (concretely `NativeBuffer<TankCommand>`
after the native-input type alias), consumed in `FixedPreUpdate` by
`update_action_state::<S>`, which writes the result into the entity's `ActionState<TankCommand>`
component before our `FixedUpdate` systems (`driving`, `shooting`, `aim`) run:

```rust
if let Some(snapshot) = input_buffer.get_predict(tick) {
    S::from_snapshot_transitions(S::State::into_inner(action_state), snapshot);
}
```
If a packet is lost/late, this system simply does nothing for that tick — **the ActionState holds
its previous value** ("equivalent to considering that the player will keep playing the last action
they played", per the source comment). This has a direct consequence for our edge-latch semantics
— see next paragraph.
source: `lightyear-src/crates/inputs/inputs/src/server.rs:683-744` (`update_action_state`)

**Edge-latch semantics (`fire_primary`) — does lightyear preserve them?** Short answer: **yes, but
only if we keep doing our own latch/consume dance identically on both client and server**, exactly
as we do today in single-player. lightyear's native input transport is a **whole-struct snapshot
per tick**, not a diff — it has no concept of "this specific field is an edge." Two consequences:

1. Our `gather_commands` (client, `RunFixedMainLoop::BeforeFixedMainLoop`, `command.fire_primary
   |= just_pressed(...)`) and `consume_edges` (client+server both, `FixedUpdate`, zeroes the flag
   after one tick sees it) logic is **entirely orthogonal to lightyear and needs zero changes** —
   lightyear just replicates whatever `TankCommand` value we hand it each tick, edge flag and all.
   The existing `command::core_plugin` (shared) + `command::client_plugin` (device gather) split
   already matches lightyear's own client/server split perfectly: `core_plugin`'s `consume_edges`
   runs on both sides identically (as it does today for single-player), and lightyear's
   `update_action_state` just becomes the mechanism that gets a remote client's `TankCommand`
   snapshot into the same `ActionState<TankCommand>` slot our systems already read from.
2. **Rollback replay risk**: because `TankCommand::fire_primary` is consumed (zeroed) within the
   same tick's `FixedUpdate` by `consume_edges`, and lightyear's rollback re-runs the *entire*
   `FixedMain` schedule (not just physics) for the replay range (see §8), a rollback that replays
   a tick where `fire_primary` was true will re-fire the gun — **if and only if** the input buffer
   for that historical tick still holds `fire_primary: true` at replay time. Since `ActionState`
   is restored from `InputBuffer` per-tick during replay (same `get_predict(tick)` call, not a
   "live" value), and the buffer stores the *original* snapshot (not the post-`consume_edges`
   zeroed value, because `ActionState` and `InputBuffer` are separate components — consuming
   edits `ActionState`, not the buffer) — replays should see the same `fire_primary: true` on the
   replayed tick and correctly re-fire deterministically. **This needs to be validated with an
   actual rollback trigger in the spike** — it is architecturally sound but not exercised by any
   example we found (none of the examples have edge-latched/consumed input fields; `simple_box`'s
   `Direction` is pure level state, `CharacterAction::Jump` in `avian_3d_character` uses
   leafwing's `just_pressed()` which is a different mechanism entirely — leafwing's `ActionState`
   tracks press/release edges natively across ticks, so it isn't a proof point for our
   consume-based approach either).

**UNCERTAIN**: whether `consume_edges` (which mutates the LIVE `ActionState<TankCommand>`
component, not the `InputBuffer`) could itself get rolled back / restored incorrectly if
`TankCommand`/`ActionState<TankCommand>` is not itself registered for prediction (`.predict()`).
Native inputs are usually NOT registered via `.component::<T>().replicate().predict()` (they ride
a separate `ActionState<A>`/`InputBuffer<A>` pathway that prediction's rollback consults directly,
per `update_action_state` re-running each rollback tick) — so this is likely fine, but the
interaction between our own same-tick mutation (`consume_edges`) and lightyear's rollback replay
of `FixedPreUpdate` (`InputSystems::UpdateActionState`) → `FixedUpdate` (our `consume_edges`,
scheduled in `GameplaySet`) should be walked through carefully in the actual spike, ordering
`consume_edges` correctly relative to `InputSystems::UpdateActionState`.

---

## 5. Replication registration

**It's a lightyear wrapper over bevy_replicon**, not raw `bevy_replicon::replicate::<T>()` calls
directly (though nothing stops you from mixing — the lightyear API explicitly documents
interop: `app.replicate::<MyComponent>()` from replicon directly, or
`app.component::<MyComponent>().predict()` to layer lightyear's prediction/interpolation
machinery on top of a replicon-registered type). lightyear 0.28's replication crate literally
imports `bevy_replicon::prelude::*` — this is the PR #1361 rewrite you were warned about.
source: `lightyear-src/crates/replication/replication/src/send.rs:19-30` (imports
`bevy_replicon::prelude::{AppRuleExt, FilterScope, Replicated, SingleComponent, VisibilityFilter}`
etc.); `lightyear-src/Cargo.toml:241` pins `bevy_replicon = { version = "0.41" }` — this **exactly
matches** the version already in the Overmatch memory note ("replicon 0.41 on Bevy 0.19 =
incumbent").

Registration API (`AppComponentExt::component::<C>()`, builder pattern):

```rust
app.component::<PlayerId>().replicate();                 // plain replication
app.component::<Position>()
    .replicate()
    .predict()                                            // rollback-eligible on Predicted entities
    .with_rollback_condition(position_should_rollback)     // custom "does this need a rollback" fn
    .add_linear_interpolation()                            // smooths Interpolated entities
    .add_linear_correction_fn();                           // smooths the rollback SNAP itself
```
source: `lightyear-src/crates/replication/replication/src/registry/replication.rs:14-107`
(`AppComponentExt` trait + doc example); `lightyear-src/examples/avian_3d_character/src/protocol.rs:88-100`
(real avian3d Position/Rotation registration, with rollback-condition closures)

**Where registration lives**: a single shared `ProtocolPlugin` mounted by BOTH client and server
apps — every example does exactly this (`examples/*/src/protocol.rs` + `shared.rs::SharedPlugin`
wrapping it), and it must be added **after** `ClientPlugins`/`ServerPlugins` and **before**
spawning the `Client`/`Server` connection entity (see §3 ordering note). This maps cleanly onto
our existing `SimPlugin` (shared authority code) — protocol registration is exactly the kind of
thing that belongs in a new module mounted by `SimPlugin`, since `SimPlugin` is already "everything
both a server and a predicting client must run."

**Replicating avian `Position`/`Rotation` for the hull**: register both, `.predict()` both, and
mount `LightyearAvianPlugin` (from `lightyear_avian3d`) which reconfigures Avian's own
Transform↔Position sync schedule ordering so it cooperates with lightyear's rollback/history/
interpolation systems — this is NOT optional glue, the plugin doc explicitly lists which stock
Avian plugins must be disabled (`PhysicsTransformPlugin`, `PhysicsInterpolationPlugin` — note this
directly conflicts with our current `PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all())`
in `src/lib.rs`, see landmines §8):

```rust
app.add_plugins(lightyear::avian3d::plugin::LightyearAvianPlugin {
    replication_mode: AvianReplicationMode::Position, // Position is source of truth, not Transform
    ..default()
});
app.add_plugins(
    PhysicsPlugins::default().build()
        .disable::<PhysicsTransformPlugin>()
        .disable::<PhysicsInterpolationPlugin>() // CONFLICTS with our existing setup — see §8
);
app.component::<Position>().replicate().predict()
    .with_rollback_condition(|a: &Position, b: &Position| (a.0 - b.0).length() >= 0.01)
    .add_linear_correction_fn().add_linear_interpolation();
app.component::<Rotation>().replicate().predict()
    .with_rollback_condition(|a: &Rotation, b: &Rotation| a.angle_between(*b) >= 0.01)
    .add_linear_correction_fn().add_linear_interpolation();
app.component::<LinearVelocity>().replicate().predict(); // no interpolation needed, not visual
app.component::<AngularVelocity>().replicate().predict();
```
source: `lightyear-src/crates/integration/avian/src/plugin.rs:1-31` (module doc, disable-list),
`:97-136` (`AvianReplicationMode` enum + `LightyearAvianPlugin` fields),
`lightyear-src/examples/avian_3d_character/src/shared.rs:71-100` (exact registration block, real 3D)

---

## 6. Spawn/possession pattern

**Entity mapping (`MapEntities`) for `Entity`-carrying components**: needed whenever a replicated
component contains an `Entity` field (our `TankRoot(Entity)` back-reference qualifies). Confirmed
required by the doc: "If the component contains any `Entity`, you need to specify how those
entities will be mapped from the remote world to the local world... Provided that your type
implements `MapEntities`... " — **but** for *components* (as opposed to messages/events), the
mapping is picked up automatically once you `impl MapEntities for T` — there is no separate
`app.component::<T>().add_map_entities()` call (that method only exists for
`app.register_message::<M>()` / `app.register_event::<E>()`, confirmed by grepping every call site
of `add_map_entities` in the workspace — all are on message/trigger/input registrations, never on
`ComponentRegistration`). `bevy_replicon` calls `C::map_entities(&mut component, ctx)`
unconditionally after deserializing any component, which is Bevy's own `MapEntities` trait method
— any component implementing it participates automatically:

```rust
#[derive(Component, Serialize, Deserialize, Clone)]
pub struct TankRoot(pub Entity);

impl MapEntities for TankRoot {
    fn map_entities<M: EntityMapper>(&mut self, entity_mapper: &mut M) {
        self.0 = entity_mapper.get_mapped(self.0);
    }
}
```
This is the exact pattern the `simple_box` example uses for its analogous `PlayerParent(Entity)`
component — it derives `MapEntities` manually and registers the component with a plain
`.replicate()`, no extra map-entities call.
source: `lightyear-src/examples/simple_box/src/protocol.rs:56-68` (`PlayerParent` + impl);
`lightyear-src/crates/replication/replication/src/registry/mod.rs:114-119` (doc: "Provided that
your type implements `MapEntities`... calling the `add_map_entities` method" — **this doc comment
is itself slightly misleading/stale-sounding**, but no such method exists on
`ComponentRegistration` in the actual source, confirmed by grep); `replicon-src/src/shared/replication/registry/rule_fns.rs:217`
(`C::map_entities(&mut component, ctx)` — the actual call site, unconditional)

**"Replicated logical entity, client-built visuals" pattern**: the exact shape we need. The
`avian_3d_character` example demonstrates it directly (closest 3D analog to our glb-scene +
binder, even though it uses primitive colliders instead of a glb):

```rust
// Server spawns the LOGICAL entity only — no mesh, no scene, just gameplay state:
commands.spawn((
    Name::new("Character"),
    ActionState::<CharacterAction>::default(),
    Position(Vec3::new(x, 3.0, z)),
    Replicate::to_clients(NetworkTarget::All),
    PredictionTarget::to_clients(NetworkTarget::All), // or Single(client_id) — see §7
    ControlledBy { owner: trigger.entity, lifetime: default() },
    CharacterPhysicsBundle::default(), // authoritative collider/rigidbody — server needs this too
    CharacterMarker,
));

// Client reacts to Added<Predicted> (or Added<Remote> for non-predicted entities) to build
// whatever is client-only (there: physics bundle; for us: spawn the glb SceneRoot as a child +
// run our existing rig binder observer on it):
fn handle_new_character(
    mut commands: Commands,
    character_query: Query<Entity, (Added<Predicted>, With<CharacterMarker>)>,
) {
    for entity in &character_query {
        commands.entity(entity).insert(CharacterPhysicsBundle::default());
        // ours: commands.entity(entity).insert(SceneRoot(tank_glb_handle));
        //       (binder observer already fires on WorldInstanceReady, unchanged)
    }
}

// Possession: gate input-marker insertion on Add<Controlled> (lightyear's own marker, distinct
// from our `tank::Controlled` — naming collision to resolve in the spike, see landmines):
fn handle_controlled_character(trigger: On<Add, lightyear::prelude::Controlled>, ...) {
    commands.entity(trigger.entity).insert(InputMarker::<TankCommand>::default());
}
```
source: `lightyear-src/examples/avian_3d_character/src/server.rs:176-191` (spawn),
`lightyear-src/examples/avian_3d_character/src/client.rs:65-107` (`handle_new_character`,
`handle_controlled_character`)

The `spaceships` demo shows the same "attach visuals as children of the replicated logical root,
reactively" pattern for a 2D mesh, confirming it generalizes: `commands.entity(entity)
.with_children(|parent| { parent.spawn((Mesh2d(...), MeshMaterial2d(...))); })`, gated on a query
filter combining `is_predicted`/`is_interpolated`/`is_prespawned`/`is_replicate`.
source: `lightyear-src/demos/spaceships/src/renderer.rs:373-465` (`insert_bullet_mesh`)

**No glb-scene / deep-hierarchy example exists anywhere in the lightyear repo** — every avian
example uses single-collider primitives (capsule, cuboid, ball). This is the single biggest gap
between the reference material and our actual use case; see §8 for the specific child-collider
mechanics that DO transfer.

---

## 7. Prediction setup

**Marking entities Predicted vs Interpolated**: server-side decision, via which `*Target`
component you attach at spawn — `PredictionTarget::to_clients(NetworkTarget)` and
`InterpolationTarget::to_clients(NetworkTarget)`. The standard "own tank predicted, others
interpolated" pattern (used identically across `simple_box`, `fps`, `lobby`, `delta_compression`,
`network_visibility`, `replication_groups`, `priority`, `distributed_authority`,
`bevy_enhanced_inputs` — i.e. essentially every multiplayer-shaped example):

```rust
commands.spawn((
    // ... tank bundle ...
    Replicate::to_clients(NetworkTarget::All),
    PredictionTarget::to_clients(NetworkTarget::Single(owner_client_id)),
    InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(owner_client_id)),
    ControlledBy { owner: link_entity, lifetime: default() },
));
```
source: `lightyear-src/examples/simple_box/src/server.rs:59-71` (canonical minimal case);
grep across `lightyear-src/examples/*/src/server.rs` — 13 of 14 multiplayer examples use this
exact `Single(id)`/`AllExceptSingle(id)` pairing (the sole outlier, `avian_3d_character`, predicts
for ALL clients including non-owners — a deliberate "let everyone predict everyone" choice, not
the norm)

**No separate `Confirmed` entity in 0.28** — this is where the book (§ "Client-side Prediction")
is **stale**: it describes a two-entity model ("a `Confirmed` entity... a `Predicted` entity...").
The actual 0.28 source has **no `Confirmed` component or entity at all** (confirmed by exhaustive
grep across the whole workspace — zero hits for `struct Confirmed`). The model is now
**single-entity**: one client-side entity carries `Remote` (bevy_replicon's own "this entity
came from a remote peer" marker, re-exported as `lightyear::prelude::client::Remote`) plus either
`Predicted` or `Interpolated`, and a per-component `ConfirmedHistory<C>` **component** (not a
second entity) stores the authoritative-state timeline used for rollback comparison.
source (no `Confirmed` anywhere): exhaustive grep, zero hits, `lightyear-src/crates/`;
`lightyear-src/crates/core/lightyear/src/lib.rs:403` (`pub use lightyear_replication::prelude::client::Remote`);
`lightyear-src/crates/replication/replication/src/lib.rs:125-126` (`pub use bevy_replicon::prelude::Remote`
— i.e. `Remote` IS bevy_replicon's own marker, re-exported, not a lightyear-specific concept);
`lightyear-src/crates/core/core/src/confirmed_history.rs:35-43` (`ConfirmedHistory<C>` is a
`#[derive(Component)]`, one per predicted/interpolated component type, living on the SAME entity)
**BOOK STALENESS FLAG**: `book/src/concepts/advanced_replication/prediction.md` describes the
old two-entity model; do not follow it literally for 0.28.

**What `lightyear_avian3d` provides for rollback**: NOT full deterministic re-simulation by
default — it's "replicate authoritative state (Position/Rotation/velocities), snap-and-replay on
mismatch." Concretely, `LightyearAvianPlugin`:
- Reorders Avian's Transform↔Position sync systems around lightyear's `PredictionSystems::UpdateHistory`
  / `FrameInterpolationSystems` / `RollbackSystems::VisualCorrection` sets so Position (not
  Transform) is authoritative and gets a `PredictionHistory` recorded every fixed tick.
- On rollback, snaps `Position`/`Rotation`/velocities (whichever you registered `.predict()` on)
  back to the confirmed value, then **re-runs the entire `FixedMain` schedule** for the replay
  range — this genuinely re-simulates physics (Avian's solver runs again each replayed tick), it
  isn't a cheap extrapolation. This is real re-simulation, just gated by which components you
  chose to make rollback-eligible.
- Optionally (`rollback_resources: true`) also rolls back Avian's own non-replicated internal
  state — `ContactGraph`, `ConstraintGraph`, `PhysicsIslands`, `ColliderAabb`/`EnlargedAabb`
  (broad-phase tree leaves), rebuilding the collider tree and repairing missing contact pairs from
  restored AABBs before replay. This flag is aimed at **deterministic replication** (inputs-only,
  no state sync) rather than our state-replication setup, and defaults to `false`.
- **Explicitly handles child colliders**: `update_child_collider_position`, run in
  `RunFixedMainLoopSystems::AfterFixedMainLoop`, recomputes every child collider's `Position`/
  `Rotation` from `parent.Position/Rotation * ColliderTransform` (the collider's fixed local
  offset) — this runs on every fixed tick, **including every tick of a rollback replay** (it's
  inside `RunFixedMainLoop`, and rollback re-runs `FixedMain`, and this specific system is outside
  `FixedMain` in `RunFixedMainLoop` which wraps it — see landmines §8 for the precise schedule
  relationship to double check). Net effect: **you only register the tank ROOT's `Position`/
  `Rotation`/velocities for prediction — never the child colliders' — and lightyear_avian3d
  reconstructs correct child-collider poses after every replay tick automatically**, matching how
  our binder already treats colliders as children with fixed local offsets from the root.
source: `lightyear-src/crates/integration/avian/src/plugin.rs:853-888`
(`update_child_collider_position` + doc comment: "In avian, this is done in
`PhysicsSystems::First`, so we need to manually run it after PhysicsSystems run to have an
accurate Position of child entities for replication"), `:355-465` (`rollback_resources` block)

**What our sim must guarantee**: everything the doc says applies directly to us —
`ServoState`/`DriveState`/`Reload` etc. must be **Components on the tank entity, never Resources**
(a `Resource` has no per-entity history and can't be snapshotted/restored per rollback the way a
`Component` can via `PredictionHistory<C>`). If they're not going over the network (purely
client/server-locally-derived, not replicated), they still need rollback participation via
`local_rollback::<C>()` — a dedicated builder method for exactly this "not networked, but needs to
be part of rollback replay" case:

```rust
app.local_rollback::<ServoState>();  // no .replicate(), no .predict() — just participates in
app.local_rollback::<DriveState>();  // rollback snapshot/restore alongside the networked ones
app.local_rollback::<Reload>();
```
source: `lightyear-src/crates/replication/prediction/src/registry.rs:697-702`
(`PredictionBuilderExt::local_rollback` — "Enable local rollback for a component or resource that
is not handled by Replicon's prediction marker writes")

**Toggling prediction OFF (pure interpolation of own tank) for feel testing**: no single
documented "prediction on/off" switch exists as a first-class toggle in the API surface we found
— it's a **spawn-time decision** (which `*Target` component the server attaches), not a runtime
flag on the client. Two ways to get the toggle:

1. **Config-gated spawn** (simplest, matches "same build, feel-test flag"): a resource/const read
   by the server's tank-spawn system, branching between
   `PredictionTarget::to_clients(Single(owner))` and `InterpolationTarget::to_clients(Single(owner))`
   for the owner's own tank (everyone else stays `InterpolationTarget` either way). This requires
   no lightyear-internal knowledge beyond what §6/§7 already cover — it's exactly the same spawn
   code, just swapping which `*Target` component goes on the owner.
2. `InputDelayConfig::no_prediction()` — a *client-global* lockstep-style knob that sets
   `maximum_predicted_ticks: 0`, forcing all latency to be covered by input delay instead of
   prediction. This is coarser (affects the whole client's input timeline, not per-entity) and is
   really a different feature (deterministic lockstep) repurposed as a blunt "no prediction"
   switch — **not recommended** for our per-tank feel-test toggle; option 1 is the right shape.
source: `lightyear-src/crates/core/sync/src/timeline/input.rs:189-196, 206-213`
(`InputDelayConfig::no_input_delay()` vs `::no_prediction()`)

**UNCERTAIN**: whether swapping `PredictionTarget` → `InterpolationTarget` on an *already-spawned,
already-predicted* entity (a true runtime toggle, no respawn/reconnect) works cleanly. The
`ReplicationTarget<T>::on_discard` hook (fires on component removal) does clean up
`ClientVisibility` bookkeeping, which suggests removal is a supported operation architecturally —
but no example exercises a live prediction↔interpolation switch, so this is inferred from the
hook's existence, not proven. **For the spike, prefer the config-gated-at-spawn approach (option
1 above) and treat a live runtime toggle as a stretch goal to validate separately**, likely via
reconnecting or respawning the tank entity between feel-test runs rather than mutating live state.
source: `lightyear-src/crates/replication/replication/src/send.rs:105-106` (`#[component(on_insert
= ..., on_discard = ...)]` on `ReplicationTarget<T>`)

---

## 8. Known landmines

**Avian3d + deep hierarchies (colliders on child entities)**: partially covered in §7 — child
collider *positions* are handled (`update_child_collider_position`), but **no example in the
lightyear repo has a scene-spawned multi-level hierarchy** (glb → binder → colliders on named
child nodes). Specific risks not covered by any source we found:
- `update_child_collider_position`'s query is `Query<(&ColliderTransform, &mut Position, &mut
  Rotation, &ColliderOf), Without<RigidBody>>` — it relies on Avian's own `ColliderOf` relationship
  component (set up automatically when a `Collider` is a descendant of a `RigidBody`), which our
  binder already produces via `ColliderConstructorHierarchy`/`ColliderConstructor` per `tank.rs`.
  This should transfer, but **has not been verified with lightyear in the loop** — the spike's
  first real test should be: does a child collider (e.g. `Turret`, `Gun`) still track correctly
  through a forced rollback when the tank root is `Predicted`.
- Our rig binder runs as an **observer on `WorldInstanceReady`** (fired once, async, after glb
  scene load) — since rollback only re-runs `FixedMain` (not `PreUpdate`/`Update`/asset systems),
  the binder observer will NOT re-fire during rollback replay (confirmed, §"rollback re-runs
  FixedMain only" below) — good, this means the binder only ever runs once per tank spawn,
  exactly as today.
- **UNCERTAIN**: whether `ColliderConstructorHierarchy`'s own async/deferred collider construction
  (which itself likely runs in a normal `Update`-adjacent schedule, not `FixedMain`) can race with
  a rollback attempting to restore/interpolate a not-yet-fully-built rig. Our existing
  spawn-before-bind race mitigation (per `headless_test.rs` comments: "the same spawn-before-bind
  race the game keeps to a frame or two") may need to extend to "don't mark `PredictionTarget`/
  `InterpolationTarget` until the rig binder has finished," which is not something any example
  demonstrates (their colliders are synchronous, same-frame `Bundle` inserts, no async gltf
  involved).

**Fixed-timestep conflicts — a REAL, already-identified conflict**: `LightyearAvianPlugin`
requires disabling Avian's own `PhysicsInterpolationPlugin` ("FrameInterpolation handles
interpolating Position and Rotation" instead). Our `src/lib.rs` currently does:
```rust
PhysicsPlugins::default().set(PhysicsInterpolationPlugin::interpolate_all())
```
with the doc comment explicitly calling this "consistent with our sim-in-fixed bet (ADR-0004)."
**This is a direct conflict** — adopting `lightyear_avian3d` means removing
`PhysicsInterpolationPlugin::interpolate_all()` and switching to lightyear's own
`FrameInterpolationSystems`/`lightyear_frame_interpolation::FrameInterpolate<Position>` for the
same "smooth rendering between fixed ticks" job. This is a straightforward swap (same visual
goal, different plugin owns it) but is a concrete code change the spike needs to make on day one,
not a hypothetical.
source: `lightyear-src/crates/integration/avian/src/plugin.rs:19-30` (module doc: "Do not forget
to disable some of the avian plugins!" — explicit code block disabling `PhysicsTransformPlugin`
and `PhysicsInterpolationPlugin`); our `src/lib.rs` (`PhysicsInterpolationPlugin::interpolate_all()`
+ ADR-0004 comment)

**`IslandPlugin`/`IslandSleepingPlugin`**: the `avian_3d_character` example explicitly disables
both ("disable Sleeping plugin as it can mess up physics rollbacks"). We don't currently disable
these (not mentioned in our `src/lib.rs` `PhysicsPlugins::default()` call, so they're on by
Avian's own defaults) — worth disabling from the start of the spike to avoid a sleeping-body
rollback bug that's already a known-enough issue to be called out in example code.
source: `lightyear-src/examples/avian_3d_character/src/shared.rs:89-97`

**WgpuSettings/headless server quirks**: no lightyear-specific gotcha found beyond what our own
`headless_test.rs` already solved (full `DefaultPlugins` + `backends: None` + no window, NOT
`MinimalPlugins` — see §2's UNCERTAIN flag, since the examples harness uses `MinimalPlugins` for
its headless servers and that may not suffice for our glb-loading server).

**Rollback re-runs the entire `FixedMain` schedule, not just physics** — confirmed directly:
```rust
for i in 0..num_rollback_ticks {
    // ...
    world.run_schedule(FixedMain); // ALL FixedUpdate systems, ours included
    // ...
}
```
This means every one of our `FixedUpdate` systems (`driving`, `shooting`, `aim`, `damage`,
`ballistics`) re-runs during a replay, which is what we want for correctness (deterministic
re-simulation) but is a landmine for **non-idempotent side effects**: spawning a shell entity,
playing a fire sound, decrementing ammo, applying a one-shot recoil impulse — anything not purely
a function of (state at tick N) → (state at tick N+1) will double-fire or corrupt state across a
rollback replay unless gated. The mitigation is `is_in_rollback` as a system run-condition,
already exported for exactly this purpose:
```rust
use lightyear_core::timeline::is_in_rollback;
app.add_systems(FixedUpdate, spawn_shell_vfx.run_if(not(is_in_rollback)));
```
This is the single most consequential landmine for our shooting/damage code specifically —
`shooting::fire` (which consumes the `fire_primary` edge and presumably spawns a shell) needs an
explicit audit for idempotency under replay before prediction is turned on for real.
source: `lightyear-src/crates/replication/prediction/src/rollback.rs:1112-1143` (`run_schedule(FixedMain)`
loop), `:64,85` (`is_in_rollback` import/re-export), used as a `run_if` gate in
`lightyear-src/crates/integration/avian/src/plugin.rs:226,232` for exactly this class of problem
(visual-only correction systems skipped during rollback)

**Naming collision**: lightyear has its own `Controlled` marker component
(`lightyear::prelude::Controlled`, inserted on the client's own predicted/owned entity) which
collides by name with our existing `tank::Controlled` (marks "the tank the player is currently
commanding," used for camera/input/HUD scoping, orthogonal to network ownership — a player could
conceivably possess a tank they don't network-own in a spectator/replay scenario, though that's
not a current feature). These are **different concepts** that happen to share a name — needs an
explicit rename on our side (or an aliased import) before the spike, not a blocker but worth
flagging before it causes a silent bug from an accidental `use` collision.
source: `lightyear-src/examples/avian_3d_character/src/client.rs:9` (`use lightyear::prelude::Controlled;`),
our `src/tank.rs:47` (`pub struct Controlled;`)

---

## Recommended spike order

Smallest compilable increments, each one validating a specific risk flagged above before building
on it:

1. **Bare connectivity, no game state.** New scratch binary (or `#[cfg(feature = "spike")]` bin
   target — do not touch `SimPlugin`/`ClientPlugin` yet): headless server (try `MinimalPlugins`
   first, matching the examples harness) + a client, both on `ServerPlugins`/`ClientPlugins` with
   `tick_duration = 1/64s`, UDP+netcode+`Authentication::Manual`, zero protocol registration beyond
   an empty `ProtocolPlugin`. Success = client's `Connected` observer fires. This validates §2/§3
   wiring in isolation, before any of our game code is in the loop.

2. **Headless server + our glb/spec loading, still no lightyear game state.** Swap the scratch
   server's plugin set from `MinimalPlugins` to our proven `DefaultPlugins` + `backends: None` +
   no-window recipe (`headless_test.rs`), confirm it STILL connects and holds the netcode
   connection while asset IO is in flight. This directly resolves the §2 UNCERTAIN flag — the
   highest-risk unknown in this whole map — before any replication code depends on it.

3. **Replicate one plain component, no prediction.** Add a trivial `Replicate::to_clients(All)`
   entity with one `.replicate()`-only component (no `.predict()`), confirm it shows up
   client-side with the `Remote` marker. Validates §5's registration ordering and the
   `AppComponentExt` API shape.

4. **`TankCommand` over `input_native`, no prediction, no physics.** Register
   `input::native::InputPlugin::<TankCommand>::default()` in the shared protocol, wire our
   existing `command::core_plugin`'s `consume_edges` unchanged, add a trivial server system that
   logs `ActionState<TankCommand>` per tick. Validates §4's edge-latch-survives-the-wire claim
   with a real click, not just static analysis — this is the first point where the edge-consume
   interaction becomes empirically checkable.

5. **Avian `Position`/`Rotation` replication on a single primitive collider (no scene, no
   hierarchy), predicted for the owner.** Mount `LightyearAvianPlugin`, disable
   `PhysicsTransformPlugin`/`PhysicsInterpolationPlugin`/`IslandPlugin`/`IslandSleepingPlugin`
   (§8), register `Position`/`Rotation`/`LinearVelocity`/`AngularVelocity` with `.predict()`.
   Drive it with `TankCommand.throttle`/`.steer` through a stub movement system (not our real
   `driving` module yet). Force a rollback (e.g. artificial latency via `RecvLinkConditioner`) and
   confirm the primitive snaps/replays correctly. Validates §5 + the first half of §7.

6. **Swap in the real glb scene + rig binder on the predicted root, primitive collider retired.**
   This is where the §8 "no example combines async scene load with prediction" gap gets tested for
   real — confirm the binder observer fires exactly once (not per rollback), and that child
   colliders (turret/gun) track correctly through a forced rollback via
   `update_child_collider_position`.

7. **Wire `SimPlugin` in for real** (replace the stub movement system from step 5 with our actual
   `driving`/`aim`/`shooting`/`damage` systems), audit `shooting::fire` and any other
   edge-triggered side-effect system for `is_in_rollback` gating (§8's non-idempotent-side-effects
   landmine) before enabling prediction on anything that fires.

8. **Add the prediction on/off toggle** (§7, option 1 — config-gated at spawn time between
   `PredictionTarget`/`InterpolationTarget` for the owner) and do the actual feel test this whole
   spike exists for.

9. **Second client (or a bot), non-owner tank interpolated.** Confirms `InterpolationTarget`
   for remote tanks, and is the first point multi-tank visibility/`ControlledBy`/entity-mapping
   (`TankRoot(Entity)` + `MapEntities`, §6) gets exercised with two real network peers instead of
   one.
