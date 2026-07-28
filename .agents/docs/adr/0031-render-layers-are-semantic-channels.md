# Render layers are semantic channels

> **Status: accepted; landed local, playtest pending. View-layer only — no wire surface, no
> `PROTOCOL_REV` movement.**

`RenderLayers` carries stable rendering DOMAINS and nothing else. Three semantic channels
(`BattlefieldWorld`, `ViewSubjectBody`, `ShadowOnly`) live behind one deep module, `render_policy`,
which is the only place in `src/` that knows a layer integer or writes a Bevy shadow marker. Every
other module declares intent — a `VisualScope` preset on an object, a `CameraProfile` on a camera, a
`LightProfile` on a light — and the module resolves those into `RenderLayers` +
`NotShadowCaster`/`NotShadowReceiver` on change.

## Context

One bitmask was doing four unrelated jobs at once: camera routing, per-view hiding, shadow
participation, and — by omission — appearance.

The per-view hiding job was the one that hurt. `sight::OPTIC_HIDDEN_LAYER = 1` was not a layer
anything drew; it was per-entity state wearing a routing component's clothes.
`reconcile_optic_render_layers` ran EVERY frame in `Update`, walked every `Mesh3d` under every
`Tank` — a Tiger is the hull plus ~194 track shoes plus the whole glb tree — and rewrote the layer
of each one, purely to keep the local hull out of the gunner optic. The cost was
O(tanks × descendants) per frame to express a fact that changes on a key press.

It was also wrong in three ways that the shape of the mechanism made hard to see:

- **The escaped turret.** The sweep walked descendants of a `Tank`. `tank::view`'s
  `detach_view_on_turret_launch` reparents a launched turret's whole view subtree OUT from under
  the tank root onto the free simulation body, so those meshes left the sweep's reach and froze at
  whatever layer they last held. Brew up while in the gunner optic and the flying turret stayed
  invisible to you for the rest of the round — visible to every other player, and to you in third
  person. Three entry points reach that state (the launch observer, the bind-time repair when the
  observer fires before the view exists, and the net client's `LaunchedTurretPose` path, which
  inserts the same marker and therefore fires the same observer).
- **Hiding and un-shadowing were the same act.** Moving a mesh off layer 0 did not merely hide it
  from one camera; in Bevy 0.19 it silently removed it from every shadow phase. The player's own
  tank stopped casting for as long as the sight was up.
- **The track shadow proxy needed an exemption and a trick.** `track::shadow_proxy`'s ribbon is a
  `Mesh3d` under the tank root, so the sweep would have swept it too — and by the point above that
  silences the controlled tank's tracks entirely. It was rescued by a `Without<TrackShadowProxy>`
  filter inside `sight` (a view module knowing what a track ribbon is) and by hiding the ribbon with
  a zero-alpha `AlphaMode::Multiply` material instead of a layer, behind ~60 lines of prose arguing
  that a layer could not work.

Industry practice has settled this. Unity splits `Camera.cullingMask` from `Light.cullingMask` and
keeps both as domains; Godot's `cull_mask` is the same idea; Unreal answers the per-view question
with per-primitive policy — `bOwnerNoSee` / `bOnlyOwnerSee` for drawing and `bCastHiddenShadow` for
casting — rather than with a layer at all. The common rules are that a layer names a domain, never
a frame's state, and that cast-versus-draw is per-object policy.

## Decision

### Channels

Three, and no speculative fourth:

| Channel            | Meaning                                                          |
|--------------------|------------------------------------------------------------------|
| `BattlefieldWorld` | everything, by default                                           |
| `ViewSubjectBody`  | the body whose view the local camera currently occupies          |
| `ShadowOnly`       | invisible caster geometry standing in for something expensive    |

The integers are private and appear exactly once, in `Channel::bit`. A source-scan test rejects
`RenderLayers::layer`, `RenderLayers::from_layers`, `RenderLayers::none` and `&mut RenderLayers`
anywhere under `src/` outside the module, with an explicit allow-list rather than a silent gap.

`ViewSubjectBody`, not `Self` or `Local`: the channel means "the body this camera is riding", which
is the right answer for a spectator or replay camera too.

### Profiles, and the invariant

`BattlefieldThirdPerson` = {World, ViewSubjectBody}. `BattlefieldOptic` = {World}. **A camera never
stops drawing `BattlefieldWorld`** — the optic drops exactly one channel. That invariant is what
lets the other ~30 view modules ignore this one entirely: nothing in `vfx`, no gizmo, no impact
marker, no prewarm rig carries a scope, so all of it is on `BattlefieldWorld` by construction and
survives every view. "Helpfully" scoping any of it is a regression, and the module doc says so.

Lights subscribe through `LightProfile`, whose variants (`BattlefieldSun`,
`BattlefieldMuzzleFlash`) currently resolve to the same all-three-channel mask. That is not an
oversight: a shadow proxy is a caster stand-in for ALL lights, and a light that skipped
`ViewSubjectBody` would stop lighting the player's own hull.

### Per-object policy

`VisualScope` pairs an optional channel with a shadow role, exposed only through named presets so
authors pick from a menu instead of composing raw values: `WORLD_SOLID`, `VIEW_SUBJECT_BODY`,
`WORLD_EFFECT`, `SHADOW_PROXY`, and `PROXIED_CASTER`. The fifth is the caster swap — geometry whose
shadow has been handed to a proxy — and it is the reason the channel is optional: a silenced track
shoe must override its shadow role while STILL inheriting its tank's channel, so it follows the
tank into and out of the optic exactly like the hull does.

`RenderLayers` does not inherit in Bevy, so the module supplies application-level inheritance:
nearest scope on the entity or an ancestor wins, resolved independently for the channel and the
shadow role. One write on a tank root moves the whole body, including glb leaves that have not
loaded yet.

Resolution runs on change only — `Added<Mesh3d>`, `Changed<VisualScope>`, `Changed`/removed
`ChildOf` — in `PostUpdate` ahead of `VisibilitySystems::CheckVisibility`, so an entity spawned
anywhere from `Startup` through the scene-spawner schedules resolves in the same frame it appears.
Steady state is three empty queries. Entering the optic is one component write on one camera.

An absent `RenderLayers` already IS `BattlefieldWorld`, so world geometry is left untouched rather
than stamped with an explicit default; the resolver costs a parent lookup per new mesh and nothing
else.

### The vendored backport this depends on

`vendor/bevy_pbr-0.19.0-scalar-math` is patched so every shadow view inherits its LIGHT's
`RenderLayers` — a backport of bevyengine/bevy#24797 (fixes #24792), merged 2026-07-08, milestone
0.19.1. Before it, shadow views carried no mask, defaulted to layer 0, and silently dropped every
caster off layer 0, which is precisely what made "hide it with a layer" un-shadow the thing.

**Retirement condition: delete the patch when Bevy 0.19.1 ships.** Upstream's shape differs (it
attaches `RenderLayers` as a component on the extracted light entity and drops
`ExtractedDirectionalLight::render_layers`); ours is smaller and changes no public API.
`tests/bevy_shadow_view_render_layers.rs` fails if a vendor refresh drops it.

The consequence to keep in mind is that **shadow correctness now depends on each LIGHT carrying the
right mask**. A light with no profile is layer-0-only and will not shadow anything moved off
`BattlefieldWorld`. Two tests pin the two ways that bites: the sun reaches every channel, and no
camera reaches `ShadowOnly`.

### One-local-player assumption

Putting the view subject on a LAYER is the Unity answer, and it carries Unity's limitation: a layer
is a global fact, so exactly one body can be the view subject at a time. That is correct for this
game — one local player, one 3D camera — and it stays correct for a spectator or replay camera,
which rides one body at a time too.

It would NOT survive split-screen. Two local cameras riding different bodies want "hide body A from
camera A and body B from camera B", which no single global bit can express. The migration then is
Unreal's, not more layers: per-primitive owner visibility, i.e. a per-view relationship rather than
a per-object channel. That is a real rewrite of this module's resolution step and a small one of its
interface — `VisualScope::VIEW_SUBJECT_BODY` would become "owned by view N" — and it is deliberately
not paid for now.

## Consequences

- `OPTIC_HIDDEN_LAYER`, `desired_optic_layer` and `reconcile_optic_render_layers` are deleted, along
  with the `Without<TrackShadowProxy>` exemption and the `TrackShadowProxy` marker itself (it had
  exactly one consumer). `sight` now writes two components: `VisualScope` on each tank root from
  control ownership, and `CameraProfile` on the one camera from `SightMode`.
- The track ribbon carries `VisualScope::SHADOW_PROXY` and an ORDINARY OPAQUE material. The
  zero-alpha `AlphaMode::Multiply` trick and its ~60 lines of justification are retired; what
  survives in `track::shadow_proxy` is the historical record of why `AlphaMode::Blend` cast NOTHING
  (its shadow item takes `MeshPipelineKey::MAY_DISCARD`, and `prepass_alpha_discard` discards under
  a 0.05 cutoff before depth is written) and a tripwire test that fails if anyone reaches for a
  transparent material here again.
- The caster swap writes `VisualScope::PROXIED_CASTER` instead of `NotShadowCaster` directly. Both
  halves of the belt's shadow — the ribbon's channel and the shoes' silencing — are now pinned by
  tests, because losing both at once has shipped before.
- The launched turret is re-scoped to `WORLD_SOLID` at both view-side entry points, and the
  `Changed<ChildOf>` half of the resolver would catch it anyway. Stated explicitly because it is a
  claim about the turret, not about the absence of a parent.
- Steady-state cost of the whole subsystem is zero; the optic switch went from
  O(tanks × descendants) per frame to O(1) on the transition.
- `damage.rs` was left alone. It is the SIM half of the turret launch and runs on the dedicated
  server, which mounts no render policy at all; the view-side observer it triggers is the correct
  place for a view component (ADR-0014).

### Deferred

- **Sandbox migration.** `src/sandbox.rs` and `src/track_sandbox/` keep their own overlay/UI layer
  stacks and are on the scan's explicit allow-list. They are dev binaries with several cameras
  each and neither mounts the resolver; migrating them buys nothing today.
- **More channels.** No garage, x-ray, outline or minimap channel is reserved. A channel with no
  camera behind it is a bitmask waiting to be misused; they arrive when a real camera or pass does.
- **Faction appearance.** Appearance-by-omission was the fourth job the bitmask was doing. It is a
  material/asset concern and does not come back as a layer.
- **Spotting / visibility gameplay.** Not a rendering channel. If it lands it is authority state
  that gates `Visibility`, not a mask.
- **`vfx` shadow markers.** `src/vfx/*` still writes `NotShadowCaster`/`NotShadowReceiver` by hand.
  Those entities are unparented world-space or children of a SHELL — never of a tank — so they are
  on `BattlefieldWorld` either way and need no channel work. `WORLD_EFFECT` is the preset they would
  migrate to; only `ballistics`' tracer streak has moved so far.

## Related

[[0014-sim-view-split]] · [[0024-one-authoritative-runtime-for-player-facing-worlds]] ·
`vendor/bevy_pbr-0.19.0-scalar-math/OVERMATCH_PATCH.md` ·
`upstream/bevy-shadow-view-ignores-light-render-layers.md`
