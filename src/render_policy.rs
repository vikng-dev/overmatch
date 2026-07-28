//! **Rendering policy** — the one module in `src/` that knows a render-layer number, and the one
//! that writes bevy's shadow markers.
//!
//! # What this replaces
//!
//! `RenderLayers` is a single bitmask that engines habitually load with four unrelated jobs at
//! once: which camera draws a thing, whether a thing is hidden from ONE view, whether a thing
//! participates in shadows, and — by omission — what a thing looks like. This repo had all four on
//! it. The worst of them was a "hide" bit dressed as a routing component: layer 1 was not a layer
//! anything drew, it was per-entity state, rewritten every frame across every mesh of every tank
//! purely to keep the local hull out of the gunner optic.
//!
//! The industry answer is the split this module implements. Unity's `Camera.cullingMask` /
//! `Light.cullingMask`, Godot's `cull_mask` and Unreal's `bOwnerNoSee` / `bCastHiddenShadow` all
//! agree on two rules:
//!
//!   1. A layer carries a stable rendering DOMAIN — a fact about the world, not about this frame.
//!   2. Cast-versus-draw is PER-OBJECT policy, not a layer.
//!
//! So: three semantic [`Channel`]s that never change meaning, chosen per-object through
//! [`VisualScope`]; cameras and lights subscribe to sets of channels through named profiles; and
//! the shadow half of the decision rides the same component instead of a second bitmask.
//!
//! # The channels
//!
//! | Channel            | What lives there                                                   |
//! |--------------------|--------------------------------------------------------------------|
//! | `BattlefieldWorld` | everything, by default — terrain, opponents, shells, every effect   |
//! | `ViewSubjectBody`  | the body whose view the local camera currently occupies             |
//! | `ShadowOnly`       | invisible caster geometry standing in for something expensive       |
//!
//! `ViewSubjectBody` is deliberately NOT called `Self` or `Local`: the channel means "the body this
//! camera is riding", which is the right answer for a future spectator or replay camera too, and
//! the wrong-but-tempting name would quietly assume the local player.
//!
//! There are no channels reserved for a garage, an x-ray view, an outline pass or a minimap. A
//! channel with no camera behind it is a bitmask waiting to be misused; they get added when a real
//! camera or pass exists to draw them.
//!
//! # The invariant worth stating out loud
//!
//! **Every camera always draws `BattlefieldWorld`.** The gunner optic does not stop drawing the
//! world; it merely stops drawing `ViewSubjectBody`. That is what makes the rest of the codebase
//! able to ignore this module completely: an effect, a gizmo, an impact marker or a prewarm rig
//! that carries no [`VisualScope`] at all is on `BattlefieldWorld` by construction and is therefore
//! visible in every view, in every mode, forever. Nothing outside a tank body needs scoping, and
//! "helpfully" scoping it is a regression.
//!
//! # Shadows depend on the LIGHT's mask now
//!
//! `vendor/bevy_pbr-0.19.0-scalar-math` is patched so that every shadow view inherits its LIGHT's
//! `RenderLayers` (a backport of bevyengine/bevy#24797, milestone 0.19.1 — see that crate's
//! `OVERMATCH_PATCH.md`). Before the patch, shadow views carried no mask at all, defaulted to layer
//! 0, and silently dropped every caster off layer 0. After it, a light with NO mask is layer-0-only
//! and will not shadow anything this module has moved off `BattlefieldWorld`.
//!
//! That is why [`LightProfile`] exists and why the sun's profile covers all three channels: it is
//! now the single point of failure for the local tank's shadow AND for the track ribbon's. Both are
//! pinned by tests below.
//!
//! # Cost
//!
//! Resolution is on CHANGE, never per frame: a new `Mesh3d`, a changed [`VisualScope`], or a
//! changed/removed `ChildOf`. Steady state is three empty queries. Entering the gunner optic is one
//! camera's mask changing — O(1), against the O(tanks × descendants) per-frame walk this replaces.

use std::collections::HashSet;

use bevy::camera::visibility::{RenderLayers, VisibilitySystems};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;

// ---------------------------------------------------------------------------------------------
// The private numbers
// ---------------------------------------------------------------------------------------------

/// A rendering DOMAIN. Stable for the life of the project; never per-entity state.
///
/// The integers are private on purpose and appear exactly once, in [`Channel::bit`]. Nothing
/// outside this module may see them — see `no_raw_render_layers_outside_this_module`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Channel {
    /// Everything, by default.
    BattlefieldWorld,
    /// The body whose view the local camera currently occupies.
    ViewSubjectBody,
    /// Invisible caster geometry — drawn by no camera, seen by every light.
    ShadowOnly,
}

impl Channel {
    /// The ONE place a render-layer integer is written down.
    const fn bit(self) -> usize {
        match self {
            Self::BattlefieldWorld => 0,
            Self::ViewSubjectBody => 1,
            Self::ShadowOnly => 2,
        }
    }

    /// The bevy mask for a set of channels.
    fn mask(channels: &[Self]) -> RenderLayers {
        channels.iter().fold(RenderLayers::none(), |mask, channel| {
            mask.with(channel.bit())
        })
    }
}

/// What an object does in the shadow passes — per-object policy, deliberately NOT a channel
/// (Unreal's `bCastHiddenShadow`, not Unity's culling mask).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ShadowRole {
    /// Ordinary solid geometry.
    CastAndReceive,
    /// Casts but is never lit — the shadow-proxy role.
    CastOnly,
    /// Drawn and lit, but its shadow is carried by something else (the caster swap).
    ReceiveOnly,
    /// Takes no part in shadows at all — light streaks, particles, debug ribbons.
    Neither,
}

impl ShadowRole {
    const fn casts(self) -> bool {
        matches!(self, Self::CastAndReceive | Self::CastOnly)
    }

    const fn receives(self) -> bool {
        matches!(self, Self::CastAndReceive | Self::ReceiveOnly)
    }
}

// ---------------------------------------------------------------------------------------------
// The interface: three components and a menu of presets
// ---------------------------------------------------------------------------------------------

/// An object's rendering policy: which [`Channel`] it is drawn on, and its [`ShadowRole`].
///
/// Construct it ONLY from the presets below — the fields are private so a caller cannot compose a
/// combination nobody has thought about. Pick from the menu; if nothing on the menu fits, the menu
/// is what needs changing.
///
/// # Inheritance
///
/// `RenderLayers` does not inherit in bevy, so this module supplies application-level inheritance:
/// a mesh takes the NEAREST scope on itself or an ancestor. That is what lets one write on a tank
/// root move ~500 meshes, and what lets a shadow-proxy child override its tank-root ancestor.
///
/// The two halves are resolved independently, so a scope may override the shadow role while
/// letting the channel keep inheriting ([`VisualScope::PROXIED_CASTER`] is exactly that). An entity
/// with no scope on itself or any ancestor is `BattlefieldWorld` + `CastAndReceive`.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct VisualScope {
    /// `None` = keep inheriting the channel from an ancestor.
    channel: Option<Channel>,
    shadow: ShadowRole,
}

impl VisualScope {
    /// Ordinary world geometry: drawn by every camera, casts and receives. The default, and the
    /// scope a tank body returns to when the local player is not riding it.
    pub(crate) const WORLD_SOLID: Self = Self {
        channel: Some(Channel::BattlefieldWorld),
        shadow: ShadowRole::CastAndReceive,
    };

    /// The body the local camera is riding. The third-person view draws it; the gunner optic does
    /// not. Everything under it inherits, which is the whole point.
    pub(crate) const VIEW_SUBJECT_BODY: Self = Self {
        channel: Some(Channel::ViewSubjectBody),
        shadow: ShadowRole::CastAndReceive,
    };

    /// A world-space visual effect: drawn everywhere, takes no part in shadows (a light streak that
    /// casts a capsule shadow across the terrain is the failure this prevents).
    pub(crate) const WORLD_EFFECT: Self = Self {
        channel: Some(Channel::BattlefieldWorld),
        shadow: ShadowRole::Neither,
    };

    /// Invisible caster geometry standing in for something too expensive to cast: no camera draws
    /// it, every light sees it. Overrides an ancestor's channel, so a proxy parented under a tank
    /// body stays a caster whichever view the player is in.
    pub(crate) const SHADOW_PROXY: Self = Self {
        channel: Some(Channel::ShadowOnly),
        shadow: ShadowRole::CastOnly,
    };

    /// Drawn geometry whose shadow has been handed to a [`VisualScope::SHADOW_PROXY`]: it stops
    /// casting but still receives, and it KEEPS INHERITING its channel — a track shoe must follow
    /// its tank into and out of the optic exactly like the hull does.
    pub(crate) const PROXIED_CASTER: Self = Self {
        channel: None,
        shadow: ShadowRole::ReceiveOnly,
    };
}

/// Which channels a camera draws. Put this on a `Camera3d` and change it to switch views; the mask
/// is resolved on change, so switching is O(1) in the number of cameras and independent of how much
/// geometry the world holds.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum CameraProfile {
    /// The free commander view: the world AND the body it is riding.
    BattlefieldThirdPerson,
    /// The gunner optic: the world, and NOT the body it is riding. Note what this does not say —
    /// the optic never stops drawing `BattlefieldWorld`; it drops exactly one channel.
    BattlefieldOptic,
}

impl CameraProfile {
    fn mask(self) -> RenderLayers {
        match self {
            Self::BattlefieldThirdPerson => {
                Channel::mask(&[Channel::BattlefieldWorld, Channel::ViewSubjectBody])
            }
            Self::BattlefieldOptic => Channel::mask(&[Channel::BattlefieldWorld]),
        }
    }
}

/// Which channels a light illuminates and casts from.
///
/// Both variants currently resolve to the SAME mask — every channel, shadow-only geometry included
/// — and that is the correct answer for every light on a battlefield: a shadow proxy is a caster
/// stand-in for ALL lights, not just the sun, and a light that skipped `ViewSubjectBody` would stop
/// lighting the player's own hull. They are two names rather than one because the call sites read
/// as declarations of what the light IS, and because the first light that legitimately needs a
/// narrower set (a garage key, a UI rig) should get its own variant instead of silently widening
/// one of these.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LightProfile {
    /// The scene's key light. Its mask is load-bearing: with the vendored shadow-view patch live,
    /// this is the only thing keeping the local tank and the track ribbon casting at all.
    BattlefieldSun,
    /// A transient muzzle flash.
    BattlefieldMuzzleFlash,
}

impl LightProfile {
    fn mask(self) -> RenderLayers {
        match self {
            Self::BattlefieldSun | Self::BattlefieldMuzzleFlash => Channel::mask(&[
                Channel::BattlefieldWorld,
                Channel::ViewSubjectBody,
                Channel::ShadowOnly,
            ]),
        }
    }
}

/// Resolve profiles and scopes into bevy's own components, before anything reads them.
///
/// `PostUpdate`, ahead of `VisibilitySystems::CheckVisibility` (and therefore ahead of
/// `bevy_light`'s caster sweep, which is ordered after it): an entity spawned anywhere from
/// `Startup` through the scene-spawner schedules is resolved in the SAME frame it appears, so
/// nothing ever renders a frame on the wrong channel.
pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PostUpdate,
        (
            resolve_camera_profiles,
            resolve_light_profiles,
            resolve_visual_scopes,
        )
            .before(VisibilitySystems::CheckVisibility),
    );
}

// ---------------------------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------------------------

fn resolve_camera_profiles(
    cameras: Query<(Entity, &CameraProfile, Option<&RenderLayers>), Changed<CameraProfile>>,
    mut commands: Commands,
) {
    for (entity, profile, current) in &cameras {
        let want = profile.mask();
        if current != Some(&want) {
            commands.entity(entity).insert(want);
        }
    }
}

fn resolve_light_profiles(
    lights: Query<(Entity, &LightProfile, Option<&RenderLayers>), Changed<LightProfile>>,
    mut commands: Commands,
) {
    for (entity, profile, current) in &lights {
        let want = profile.mask();
        if current != Some(&want) {
            commands.entity(entity).insert(want);
        }
    }
}

/// Walk `entity` and its ancestors for the nearest declared channel and the nearest shadow role.
///
/// The two are collected independently so [`VisualScope::PROXIED_CASTER`] can override one and keep
/// inheriting the other.
fn resolve(
    entity: Entity,
    scopes: &Query<&VisualScope>,
    parents: &Query<&ChildOf>,
) -> (Channel, ShadowRole) {
    let mut node = Some(entity);
    let mut channel = None;
    let mut shadow = None;
    while let Some(current) = node {
        if let Ok(scope) = scopes.get(current) {
            shadow = shadow.or(Some(scope.shadow));
            channel = channel.or(scope.channel);
            if channel.is_some() && shadow.is_some() {
                break;
            }
        }
        node = parents.get(current).ok().map(ChildOf::parent);
    }
    (
        channel.unwrap_or(Channel::BattlefieldWorld),
        shadow.unwrap_or(ShadowRole::CastAndReceive),
    )
}

/// Re-resolve every mesh whose answer could have changed this frame.
///
/// The dirty set is three sources, and each one is a bug that has actually shipped somewhere:
/// `Added<Mesh3d>` (a glb that arrives asynchronously over many frames), `Changed<VisualScope>` (a
/// tank the local player just took control of, with no view-mode event anywhere), and
/// `Changed`/`Removed` `ChildOf` (a turret that has just been blown off its own hull and is no
/// longer part of anybody's body).
fn resolve_visual_scopes(
    added_meshes: Query<Entity, Added<Mesh3d>>,
    changed_scopes: Query<Entity, Changed<VisualScope>>,
    changed_parents: Query<Entity, Changed<ChildOf>>,
    mut unparented: RemovedComponents<ChildOf>,
    children: Query<&Children>,
    scopes: Query<&VisualScope>,
    parents: Query<&ChildOf>,
    meshes: Query<
        (
            Option<&RenderLayers>,
            Has<NotShadowCaster>,
            Has<NotShadowReceiver>,
        ),
        With<Mesh3d>,
    >,
    mut commands: Commands,
) {
    let mut dirty: HashSet<Entity> = added_meshes.iter().collect();
    // A scope or a parent changing re-resolves the whole subtree under it: the write is on the
    // root, the consequence is every mesh below.
    let seed = |root: Entity, dirty: &mut HashSet<Entity>| {
        if meshes.contains(root) {
            dirty.insert(root);
        }
        for descendant in children.iter_descendants(root) {
            if meshes.contains(descendant) {
                dirty.insert(descendant);
            }
        }
    };
    for entity in &changed_scopes {
        seed(entity, &mut dirty);
    }
    for entity in &changed_parents {
        seed(entity, &mut dirty);
    }
    for entity in unparented.read() {
        seed(entity, &mut dirty);
    }

    for entity in dirty {
        let Ok((current, no_caster, no_receiver)) = meshes.get(entity) else {
            continue;
        };
        let (channel, shadow) = resolve(entity, &scopes, &parents);
        let want = Channel::mask(&[channel]);
        // An absent `RenderLayers` already IS `BattlefieldWorld`, so the overwhelming majority of
        // meshes in the scene — every effect, every piece of terrain — is left untouched rather
        // than paying an archetype move to say what bevy already assumes.
        let write = match current {
            None => want != Channel::mask(&[Channel::BattlefieldWorld]),
            Some(current) => *current != want,
        };
        if write {
            commands.entity(entity).insert(want);
        }
        if shadow.casts() == no_caster {
            if no_caster {
                commands.entity(entity).remove::<NotShadowCaster>();
            } else {
                commands.entity(entity).insert(NotShadowCaster);
            }
        }
        if shadow.receives() == no_receiver {
            if no_receiver {
                commands.entity(entity).remove::<NotShadowReceiver>();
            } else {
                commands.entity(entity).insert(NotShadowReceiver);
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The test surface
// ---------------------------------------------------------------------------------------------

/// Whether `viewer` (a camera or a light) reaches `target` — the resolved mask intersection, which
/// is the ONLY question anything outside this module should ever ask about a layer.
///
/// Test-only by design: production code states intent (a profile, a scope) and never inspects the
/// consequence. Callers' tests get to assert the LAW ("the optic does not draw your own hull")
/// instead of a bit value, which is what keeps those tests alive across a renumbering.
#[cfg(test)]
pub(crate) fn reaches(world: &World, viewer: Entity, target: Entity) -> bool {
    let mask = |entity: Entity| {
        world
            .entity(entity)
            .get::<RenderLayers>()
            .cloned()
            .unwrap_or_default()
    };
    mask(viewer).intersects(&mask(target))
}

/// Whether `mesh` takes part in the shadow pass as a caster.
#[cfg(test)]
pub(crate) fn casts_shadow(world: &World, mesh: Entity) -> bool {
    !world.entity(mesh).contains::<NotShadowCaster>()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world with one camera per profile, the sun, and one mesh per scope — the shape every law
    /// below is stated against.
    struct Fixture {
        app: App,
        third_person: Entity,
        optic: Entity,
        sun: Entity,
        world_solid: Entity,
        view_subject: Entity,
        shadow_proxy: Entity,
        effect: Entity,
        unscoped: Entity,
    }

    impl Fixture {
        fn new() -> Self {
            let mut app = App::new();
            app.add_plugins(plugin);
            let world = app.world_mut();
            let third_person = world.spawn(CameraProfile::BattlefieldThirdPerson).id();
            let optic = world.spawn(CameraProfile::BattlefieldOptic).id();
            let sun = world.spawn(LightProfile::BattlefieldSun).id();
            let mesh = |world: &mut World, scope: VisualScope| {
                world.spawn((Mesh3d(Handle::default()), scope)).id()
            };
            let world_solid = mesh(world, VisualScope::WORLD_SOLID);
            let view_subject = mesh(world, VisualScope::VIEW_SUBJECT_BODY);
            let shadow_proxy = mesh(world, VisualScope::SHADOW_PROXY);
            let effect = mesh(world, VisualScope::WORLD_EFFECT);
            let unscoped = world.spawn(Mesh3d(Handle::default())).id();
            app.update();
            Self {
                app,
                third_person,
                optic,
                sun,
                world_solid,
                view_subject,
                shadow_proxy,
                effect,
                unscoped,
            }
        }

        fn reaches(&self, viewer: Entity, target: Entity) -> bool {
            super::reaches(self.app.world(), viewer, target)
        }
    }

    /// The third-person view draws the world AND the body it is riding.
    #[test]
    fn third_person_draws_the_world_and_the_view_subject() {
        let f = Fixture::new();
        assert!(f.reaches(f.third_person, f.world_solid));
        assert!(f.reaches(f.third_person, f.view_subject));
        assert!(f.reaches(f.third_person, f.unscoped));
        assert!(f.reaches(f.third_person, f.effect));
    }

    /// The optic drops exactly ONE channel. It never stops drawing the world — everything that
    /// carries no scope at all (every effect, gizmo and impact marker in the game) depends on it.
    #[test]
    fn the_optic_drops_the_view_subject_and_nothing_else() {
        let f = Fixture::new();
        assert!(
            !f.reaches(f.optic, f.view_subject),
            "the optic must not draw the body it is riding — that is what it is for"
        );
        assert!(
            f.reaches(f.optic, f.world_solid),
            "the optic draws the world"
        );
        assert!(
            f.reaches(f.optic, f.unscoped),
            "an unscoped mesh is world geometry and stays visible in the optic"
        );
        assert!(
            f.reaches(f.optic, f.effect),
            "effects are never scoped and must survive every view"
        );
    }

    /// The sun reaches all three channels. With the vendored shadow-view patch live this is the
    /// single point of failure for the local tank's shadow AND the track ribbon's — the ribbon
    /// half is the regression that shipped once already (the shoes go `NotShadowCaster` on the
    /// frame the ribbon appears, so if the sun cannot see the ribbon, BOTH halves go silent).
    #[test]
    fn the_sun_reaches_every_channel() {
        let f = Fixture::new();
        assert!(f.reaches(f.sun, f.world_solid));
        assert!(
            f.reaches(f.sun, f.view_subject),
            "the local tank casts no shadow in ANY view if this fails"
        );
        assert!(
            f.reaches(f.sun, f.shadow_proxy),
            "the tracks lose their shadow entirely if this fails"
        );
    }

    /// No camera draws shadow-only geometry. This is what lets the ribbon carry an ORDINARY OPAQUE
    /// material instead of an alpha trick.
    #[test]
    fn no_camera_draws_shadow_only_geometry() {
        let f = Fixture::new();
        for camera in [f.third_person, f.optic] {
            assert!(!f.reaches(camera, f.shadow_proxy));
        }
    }

    /// The shadow roles land as bevy markers.
    #[test]
    fn shadow_roles_resolve_to_markers() {
        let f = Fixture::new();
        let world = f.app.world();
        assert!(casts_shadow(world, f.world_solid));
        assert!(!world.entity(f.world_solid).contains::<NotShadowReceiver>());
        assert!(
            casts_shadow(world, f.shadow_proxy),
            "a shadow proxy exists to cast"
        );
        assert!(
            world.entity(f.shadow_proxy).contains::<NotShadowReceiver>(),
            "a shadow proxy is never lit — it would stripe the geometry it stands in for"
        );
        assert!(!casts_shadow(world, f.effect));
        assert!(world.entity(f.effect).contains::<NotShadowReceiver>());
    }

    /// A camera's mask follows its profile, and the switch is one write on one entity.
    #[test]
    fn switching_a_camera_profile_switches_what_it_draws() {
        let mut f = Fixture::new();
        assert!(f.reaches(f.third_person, f.view_subject));
        *f.app
            .world_mut()
            .entity_mut(f.third_person)
            .get_mut::<CameraProfile>()
            .expect("the camera keeps its profile") = CameraProfile::BattlefieldOptic;
        f.app.update();
        assert!(!f.reaches(f.third_person, f.view_subject));
    }

    /// Inheritance: a mesh takes the nearest ancestor's scope, and a scope on the mesh itself wins.
    #[test]
    fn scopes_inherit_from_the_nearest_ancestor() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        let root = world.spawn(VisualScope::VIEW_SUBJECT_BODY).id();
        let node = world.spawn(ChildOf(root)).id();
        let nested = world.spawn((Mesh3d(Handle::default()), ChildOf(node))).id();
        let override_child = world
            .spawn((
                Mesh3d(Handle::default()),
                VisualScope::SHADOW_PROXY,
                ChildOf(root),
            ))
            .id();
        let camera = world.spawn(CameraProfile::BattlefieldOptic).id();
        app.update();

        assert!(
            !reaches(app.world(), camera, nested),
            "a mesh two levels down inherits the root's channel"
        );
        assert!(
            !reaches(app.world(), camera, override_child),
            "a shadow proxy overrides its ancestor rather than inheriting"
        );
        assert!(
            casts_shadow(app.world(), override_child),
            "and keeps its own casting role"
        );
    }

    /// `PROXIED_CASTER` overrides the shadow role while STILL inheriting the channel — a silenced
    /// track shoe must follow its tank into the optic exactly like the hull does.
    #[test]
    fn a_proxied_caster_keeps_inheriting_its_channel() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        let root = world.spawn(VisualScope::VIEW_SUBJECT_BODY).id();
        let shoe = world
            .spawn((
                Mesh3d(Handle::default()),
                VisualScope::PROXIED_CASTER,
                ChildOf(root),
            ))
            .id();
        let optic = world.spawn(CameraProfile::BattlefieldOptic).id();
        let third_person = world.spawn(CameraProfile::BattlefieldThirdPerson).id();
        app.update();

        assert!(
            !reaches(app.world(), optic, shoe),
            "a silenced shoe still hides from the optic with the rest of its tank"
        );
        assert!(
            reaches(app.world(), third_person, shoe),
            "and is still drawn in third person"
        );
        assert!(
            !casts_shadow(app.world(), shoe),
            "its shadow belongs to the ribbon now"
        );
        assert!(
            !app.world().entity(shoe).contains::<NotShadowReceiver>(),
            "it still RECEIVES — the hull's shadow crossing the return run is visible"
        );
    }

    /// Late arrival: a mesh attached long after its ancestor was scoped still resolves. The tank
    /// glb lands asynchronously over many frames, which is exactly why the mechanism this replaces
    /// was a per-frame sweep.
    #[test]
    fn a_late_mesh_resolves_against_an_already_scoped_ancestor() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let root = app.world_mut().spawn(VisualScope::VIEW_SUBJECT_BODY).id();
        let optic = app.world_mut().spawn(CameraProfile::BattlefieldOptic).id();
        app.update();

        let late = app
            .world_mut()
            .spawn((Mesh3d(Handle::default()), ChildOf(root)))
            .id();
        app.update();
        assert!(!reaches(app.world(), optic, late));
    }

    /// Re-parenting out from under a scoped ancestor re-resolves the whole subtree — the launched
    /// turret. Without this the escaped meshes freeze at whatever they last held and a turret blown
    /// off while the player is in the optic stays invisible forever.
    #[test]
    fn reparenting_out_of_a_scope_re_resolves_the_subtree() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let world = app.world_mut();
        let root = world.spawn(VisualScope::VIEW_SUBJECT_BODY).id();
        let subtree = world.spawn(ChildOf(root)).id();
        let mesh = world
            .spawn((Mesh3d(Handle::default()), ChildOf(subtree)))
            .id();
        let free_body = world.spawn_empty().id();
        let optic = world.spawn(CameraProfile::BattlefieldOptic).id();
        app.update();
        assert!(!reaches(app.world(), optic, mesh));

        app.world_mut()
            .entity_mut(subtree)
            .insert(ChildOf(free_body));
        app.update();
        assert!(
            reaches(app.world(), optic, mesh),
            "an escaped subtree is world geometry again"
        );
    }

    /// Steady state costs nothing: with no change, no write happens. Asserted through change
    /// detection rather than a timer, because the whole point of the rewrite is that the per-frame
    /// walk is gone.
    #[test]
    fn a_quiet_frame_writes_nothing() {
        let mut f = Fixture::new();
        f.app.update();
        let quiet_since = f.app.world().read_change_tick();
        f.app.update();
        let now = f.app.world().read_change_tick();
        // An absent mask already IS `BattlefieldWorld` — world geometry never pays an archetype
        // move to say so, which is what keeps this resolver off the hot path of a scene full of
        // effects and terrain.
        for entity in [f.world_solid, f.effect, f.unscoped] {
            assert!(
                f.app.world().entity(entity).get::<RenderLayers>().is_none(),
                "world geometry is left alone rather than stamped with the default"
            );
        }
        for entity in [f.view_subject, f.shadow_proxy] {
            let ticks = f
                .app
                .world()
                .entity(entity)
                .get_change_ticks::<RenderLayers>()
                .expect("a scoped mesh carries a resolved mask");
            assert!(
                !ticks.is_changed(quiet_since, now),
                "a quiet frame must not rewrite {entity:?}'s mask"
            );
        }
    }

    /// **The monopoly.** Render-layer numbers exist in this module and nowhere else in `src/`.
    ///
    /// Scans source text rather than trusting review: the whole value of the redesign is that
    /// "just put it on layer 3" is not a thing anyone can do from a call site any more.
    #[test]
    fn no_raw_render_layers_outside_this_module() {
        /// Files allowed to build masks by hand. Both are dev binaries with their own overlay/UI
        /// camera stacks and a deliberately DEFERRED migration (see ADR-0031); they are not
        /// shipped, and neither mounts [`plugin`]. This list is the exemption — a silent gap in
        /// the scan would not be.
        const ALLOWED: &[&str] = &[
            "src/render_policy.rs",
            "src/sandbox.rs",
            "src/track_sandbox/",
        ];
        /// Ways to author or mutate a mask. `&mut RenderLayers` is here because a query that can
        /// write one is the same authority as constructing one.
        const FORBIDDEN: &[&str] = &[
            "RenderLayers::layer",
            "RenderLayers::from_layers",
            "RenderLayers::none",
            "&mut RenderLayers",
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut offences = Vec::new();
        let mut stack = vec![root.clone()];
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
                    .strip_prefix(root.parent().expect("src/ has a parent"))
                    .expect("every scanned path is under the manifest")
                    .to_string_lossy()
                    .replace('\\', "/");
                if ALLOWED.iter().any(|allow| relative.starts_with(allow)) {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("a readable source file");
                for needle in FORBIDDEN {
                    if source.contains(needle) {
                        offences.push(format!("{relative}: {needle}"));
                    }
                }
            }
        }
        assert!(
            offences.is_empty(),
            "render-layer numbers belong to render_policy alone — declare a VisualScope, a \
             CameraProfile or a LightProfile instead:\n  {}",
            offences.join("\n  ")
        );
    }
}
