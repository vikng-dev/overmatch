//! The x-ray: a tank in the glass look on the battlefield, the latest shot that struck it drawn
//! over it, its key ([`XRay`], X) a preference kept across lives. Its own presentation beside the
//! corner slot: no corner, no stage copy, no exhibit. The look goes on the tank's REAL render
//! primitives — each mesh's `StandardMaterial` handle set aside ([`Dressed`]) and replaced by its
//! tier's [`LookMaterial`], a mask copy spawned under it — and the record's elements hang under the
//! live hull and component meshes ([`spawn_glass`]), so the pose, the servos, the certified LOD and
//! the pooled shoes follow for free; nothing is hidden and nothing is copied. One mask set is owned
//! for the player's camera ([`MaskNeed`]). While on, the tank casts no shadow (`LookMaterial` casts
//! none).
//!
//! * **The subject.** Which tank, and whose records, the composition that mounts the x-ray chooses
//!   ([`XRaySubject`]): the player's own tank under its victim records (the game's, the default),
//!   or the tank of a combatant the player fires at under the player's shooter records of hits on
//!   it (the armour sandbox's). Everything below holds for either; [`Subject`] is the only reader
//!   of the choice.
//! * **When it draws.** The preference on, the subject drawable (the player's own tank: a life,
//!   not knocked out; a struck tank: its current life, knocked out too; either with its rig
//!   bound), the third-person view and no kill cam run ([`drive`]). Every deactivation — the key,
//!   a knockout of the player's tank or the run it starts, the gunner's optic, a new tank, a
//!   disconnect — restores every handle and removes every tag and child at once ([`restore`]);
//!   the preference stays, so the x-ray returns on the next tank. A composition that despawns the
//!   subject itself lets go first ([`XRayRelease`]), ordered after the pen cam's systems
//!   ([`PenCamSet`](super::PenCamSet)) so nothing the x-ray queued that frame lands on it after.
//! * **The tint ends on the live state.** Each component's fill settles on its live HP points over
//!   its pool, out of action as live has it ([`LiveState::boxes`]); a record's doses and
//!   consequences never reach it. A component the record reaches starts from its HP points at the
//!   shot, never fewer than its live points, and rises to that end at the record's reach time, as
//!   the victim frame fills it; any other — one the record does not reach or does not name, or
//!   every one with no record — holds its live state from the first frame. A component no point of
//!   which is lost is the base layout, untinted.
//! * **The latest rule.** Of the subject's records of its current life whose impact is no longer
//!   waiting, strictly the newest by recency ([`latest`], [`latest_shot`]; a clean ricochet
//!   replaces an earlier penetration — a bounce is informative too). With none, the live tint
//!   alone.
//! * **The clock.** A record plays once: from the moment it first shows (X on with it the latest,
//!   or its arrival while on), as the victim frame plays it, to its hold beat. Every later showing
//!   of the same record — X off and on, the optic and back, a resize, an older record back as the
//!   latest — continues that clock ([`XRayState::shown`]), so past the hold beat it is the held end; a
//!   record that lands while the x-ray is off waits for X. A connection forgets every clock with
//!   the store ([`forget_shown`]). The tint's end follows the live state in the same showing.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use bevy::camera::visibility::VisibilityRange;
use bevy::ecs::query::QueryData;
use bevy::ecs::system::SystemParam;
use bevy::gltf::{GltfMaterialName, GltfMeshName};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};
use bevy::world_serialization::{WorldAsset, WorldAssetRoot, WorldInstanceReady};

use super::super::Knobs;
use super::super::look::{self, Layer, LookMaterial, MaskNeed, MaskSet, Masks};
use super::super::material::{GlowMaterial, OutlineMaterial};
use super::super::plan::{BoxPlan, Plan};
use super::super::stage::{
    self, BoxTint, FieldVolume, LineSeg, LookCtx, MarkItem, Piece, Playing, SHELL_SCENE,
    component_meshes, mask_copy, round_copies, spawn_glass,
};
use super::super::timeline::CornerKind;
use super::super::view::{self, Tier};
use super::NewMatch;
use super::killcam::Victim;
use super::{Recency, tank_asset};
use crate::bake::TankBlueprint;
use crate::ballistics::ComponentHealth;
use crate::command::Bindings;
use crate::damage::{Ammo, CrewStation, Dead, FunctionRole, TankKnockedOut, TankVolumes};
use crate::damage_cam::delivery::{
    ExplanationKey, ImpactPresence, ShotExplanations, StoredExplanation,
};
use crate::damage_cam::shapes::{TankAsset, TankShapes};
use crate::damage_cam::wire::{Audience, TankLife};
use crate::damage_cam::{PenetrationReport, Role};
use crate::geometry_lod::{GeometryLodScanned, SourceDetailOnly};
use crate::render_policy::VisualScope;
use crate::sight::SightMode;
use crate::spec::TankSpecHandle;
use crate::tank::{Controlled, Rig as TankRig, Tank, ViewNode};
use crate::view::PlayerView;
use crate::{CombatantId, CombatantLife};

/// The player's x-ray preference, which [`Bindings::xray`] toggles: kept across lives,
/// respawn, the map and a disconnect, so the x-ray returns on the next live tank.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct XRay {
    pub(crate) on: bool,
}

/// Whose tank the x-ray dresses and whose records it draws over it, set by the composition that
/// mounts it (module doc).
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum XRaySubject {
    /// The player's tank ([`Controlled`]) under its victim records, while it is not knocked out:
    /// knocked out, the death flow owns the view.
    #[default]
    OwnTank,
    /// The [`Tank`] of this combatant under the player's shooter records of hits on its current
    /// life, knocked out as well as live.
    Struck(CombatantId),
}

/// A candidate subject tank: who it is, its components, its file, its rig and its knockout.
type SubjectRow = (
    Entity,
    &'static CombatantId,
    &'static CombatantLife,
    Option<&'static TankVolumes>,
    Option<&'static TankSpecHandle>,
    Option<&'static TankRig>,
    Has<TankKnockedOut>,
);

/// The x-ray's subject as [`XRaySubject`] chooses it: the tank, its life, its file, its live state
/// and the record drawn over it.
#[derive(SystemParam)]
pub(super) struct Subject<'w, 's> {
    choice: Res<'w, XRaySubject>,
    own: Query<'w, 's, SubjectRow, With<Controlled>>,
    tanks: Query<'w, 's, SubjectRow, With<Tank>>,
    volumes: Query<'w, 's, LiveVolume>,
}

impl Subject<'_, '_> {
    /// The subject tank while the x-ray may draw it: the player's own, exactly one and not knocked
    /// out; the struck combatant's, exactly one, knocked out or not.
    fn tank(
        &self,
    ) -> Option<(
        Entity,
        &CombatantId,
        &CombatantLife,
        Option<&TankVolumes>,
        Option<&TankSpecHandle>,
        Option<&TankRig>,
        bool,
    )> {
        match *self.choice {
            XRaySubject::OwnTank => self.own.single().ok().filter(|row| !row.6),
            XRaySubject::Struck(combatant) => {
                let mut matching = self.tanks.iter().filter(|row| *row.1 == combatant);
                let row = matching.next()?;
                matching.next().is_none().then_some(row)
            }
        }
    }

    /// Whether the key reaches the preference: never while the player's own tank is knocked out
    /// or absent ([`toggle`]); a struck subject leaves the player's device alone.
    fn keyed(&self) -> bool {
        match *self.choice {
            XRaySubject::OwnTank => self.own.iter().any(|row| !row.6),
            XRaySubject::Struck(_) => true,
        }
    }

    /// The subject's life.
    fn life(&self) -> Option<TankLife> {
        let (_, &combatant, &life, ..) = self.tank()?;
        Some(TankLife { combatant, life })
    }

    /// The subject's body — its entity and rig — once its rig is bound.
    fn body(&self) -> Option<(Entity, &TankRig)> {
        let (entity, _, _, _, _, rig, _) = self.tank()?;
        Some((entity, rig?))
    }

    /// The subject's tank file, as a record names it.
    fn vehicle(&self) -> Option<String> {
        let (_, _, _, _, spec, _, _) = self.tank()?;
        let path = spec?.0.path()?;
        Some(path.path().to_string_lossy().into_owned())
    }

    /// The subject's components as they stand.
    fn live(&self) -> LiveState {
        self.tank()
            .and_then(|(_, _, _, volumes, _, _, _)| volumes)
            .map(|volumes| LiveState::read(volumes, &self.volumes))
            .unwrap_or_default()
    }

    /// The record drawn over the subject's life `life`, by the latest rule: of its victim records
    /// for the player's own tank ([`latest`]), of the player's shooter records for a struck one
    /// ([`latest_shot`]).
    fn record<'a>(
        &self,
        store: &'a ShotExplanations,
        life: TankLife,
    ) -> Option<(ExplanationKey, &'a StoredExplanation)> {
        match *self.choice {
            XRaySubject::OwnTank => latest(store, life),
            XRaySubject::Struck(_) => latest_shot(store, life),
        }
    }
}

/// Flip [`XRay`] on the key, while the cursor is captured (a menu or a pause takes the key)
/// and the subject takes it ([`Subject::keyed`]): the player's own tank knocked out, the death
/// flow owns the device and a press changes nothing, so a kill cam run never flips it.
pub(super) fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Option<Res<Bindings>>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    subject: Subject,
    mut xray: ResMut<XRay>,
) {
    let pressed = bindings.is_some_and(|bindings| bindings.xray.just_pressed(&keys, &mouse));
    if pressed && subject.keyed() && crate::state::cursor_locked(cursor) {
        xray.on = !xray.on;
    }
}

/// An entity the x-ray spawned — a dressed mesh's mask copy, a record element, the shell —
/// despawned whole on restore. Never under another: a copy hangs under a dressed mesh, an element
/// under the live hull or a dressed mesh.
#[derive(Component, Clone, Copy)]
pub(super) struct XRayItem;

/// A live render primitive wearing the look: the material it wore, restored exactly, and the names
/// of its node chain up to the tank's root, nearest first, by which the record's components find
/// their meshes.
#[derive(Component)]
pub(super) struct Dressed {
    original: Handle<StandardMaterial>,
    names: Vec<String>,
}

/// The x-ray's mask copy of a dressed mesh, drawn for the mask cameras alone: it follows the
/// source's mesh handle ([`follow_mesh`]), which a shoe's belt swaps.
#[derive(Component)]
pub(super) struct MaskCopy {
    of: Entity,
}

impl MaskCopy {
    /// The dressed mesh this is a copy of.
    #[cfg(test)]
    pub(super) fn of(&self) -> Entity {
        self.of
    }
}

/// The x-ray's shell model, the round resting where the record ends. Its meshes' mask copies hang
/// under them ([`round_landed`]), untagged, and go with it.
#[derive(Component)]
pub(super) struct XRayShell;

/// What the x-ray stands on between frames.
#[derive(Resource, Default)]
pub(super) struct XRayState {
    built: Option<Built>,
    /// App seconds each record first showed, for every record shown since the connection: a
    /// record's clock, kept across the x-ray's deactivations so it plays once, and per record
    /// because the store evicts by fire tick while [`latest`] picks by recency, so an older record
    /// can come back as the latest. A connection clears it ([`forget_shown`]): a new match's keys
    /// start over.
    shown: BTreeMap<ExplanationKey, f64>,
    /// Each tank file's asset, read once.
    tanks: HashMap<String, Arc<TankAsset>>,
    /// The shell model, loaded once the x-ray first draws a record.
    shell: Option<Handle<WorldAsset>>,
    /// The shell model's material, which a burst's pieces wear: loaded once the x-ray first draws
    /// a burst's pieces.
    shell_material: Option<Handle<StandardMaterial>>,
}

impl XRayState {
    /// The plan the x-ray draws, while built.
    #[cfg(test)]
    pub(super) fn plan(&self) -> Option<&Plan> {
        self.built.as_ref().map(|built| &*built.plan)
    }

    /// App seconds the record on show first showed, while built.
    #[cfg(test)]
    pub(super) fn since(&self) -> Option<f64> {
        self.built.as_ref().map(|built| built.since)
    }

    /// The record on show, while built with one.
    #[cfg(test)]
    pub(super) fn record(&self) -> Option<ExplanationKey> {
        self.built.as_ref().and_then(|built| built.record)
    }

    /// Draw `shell` as the round's model, in place of the game's.
    #[cfg(test)]
    pub(super) fn stand_in_shell(&mut self, shell: Handle<WorldAsset>) {
        self.shell = Some(shell);
    }
}

/// The x-ray as built: what it was built from, and the materials it dressed the tank in.
struct Built {
    tank: Entity,
    /// The live hull's view node: the record's frame.
    hull: Entity,
    /// The mask set its materials hold: a new set rebuilds everything.
    masks: MaskSet,
    record: Option<ExplanationKey>,
    live: LiveState,
    /// App seconds the record on show first showed: its elements play from there.
    since: f64,
    plan: Arc<Plan>,
    /// The record's field maps ([`super::super::field::atlas`]), which its footprints read; `None`
    /// where it draws no field.
    atlas: Option<Handle<Image>>,
    /// One material per tier the tank's meshes wear, and the layer each is masked in.
    tiers: HashMap<Tier, (Handle<LookMaterial>, Layer)>,
    /// The masks' material, on every mask copy.
    mask: Handle<LookMaterial>,
    /// Meshes were dressed this frame: the record's elements are built again next frame, once
    /// their tints can hang under them.
    settle: bool,
    /// A primitive was left undressed for the LOD ladder's scan ([`dress`]): looked at again next
    /// frame, until none is.
    rescan: bool,
}

/// The tank's hierarchy and the classification reads.
#[derive(SystemParam)]
pub(super) struct Hierarchy<'w, 's> {
    children: Query<'w, 's, &'static Children>,
    parents: Query<'w, 's, &'static ChildOf>,
    names: Query<'w, 's, &'static Name>,
    material_names: Query<'w, 's, &'static GltfMaterialName>,
    primitives: Query<'w, 's, (), With<GltfMeshName>>,
    scanned: Query<'w, 's, (), With<GeometryLodScanned>>,
    /// Present, no LOD ladder scans a primitive: each keeps its source mesh.
    source_only: Option<Res<'w, SourceDetailOnly>>,
    scopes: Query<'w, 's, &'static VisualScope>,
    view_nodes: Query<'w, 's, &'static ViewNode>,
    added: Query<'w, 's, Entity, Added<Mesh3d>>,
}

/// What the x-ray holds in the world: the dressed meshes, its items, and what is still to dress.
#[derive(SystemParam)]
pub(super) struct Held<'w, 's> {
    undressed: Query<
        'w,
        's,
        (
            Entity,
            &'static Mesh3d,
            &'static MeshMaterial3d<StandardMaterial>,
            Option<&'static VisibilityRange>,
        ),
        (Without<Dressed>, Without<XRayItem>),
    >,
    dressed: Query<'w, 's, (Entity, &'static Dressed, &'static Mesh3d)>,
    items: Query<'w, 's, Entity, With<XRayItem>>,
    elements: Query<'w, 's, Entity, (With<XRayItem>, Without<MaskCopy>)>,
}

impl Held<'_, '_> {
    /// Put every dressed mesh back in the material it wore and despawn every item of the x-ray.
    fn restore(&self, commands: &mut Commands) {
        for (entity, dressed, _) in &self.dressed {
            commands
                .entity(entity)
                .remove::<(MeshMaterial3d<LookMaterial>, Dressed)>()
                .insert(MeshMaterial3d(dressed.original.clone()));
        }
        for item in &self.items {
            commands.entity(item).despawn();
        }
    }
}

/// The x-ray as a composition that despawns its subject reads and lets go of it, from a system
/// ordered after [`PenCamSet`](super::PenCamSet). Without the pen cam mounted it reads nothing and
/// lets go of nothing.
#[derive(SystemParam)]
pub(crate) struct XRayRelease<'w, 's> {
    state: Option<ResMut<'w, XRayState>>,
    held: Held<'w, 's>,
}

impl XRayRelease<'_, '_> {
    /// Restore every dressed handle and despawn every item, with `commands`, ahead of whatever
    /// despawns the subject in them; the next frame dresses afresh whatever subject stands.
    pub(crate) fn release(&mut self, commands: &mut Commands) {
        self.held.restore(commands);
        if let Some(state) = self.state.as_mut() {
            state.built = None;
        }
    }
}

/// The x-ray's material stores.
#[derive(SystemParam)]
pub(super) struct Stores<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    glows: ResMut<'w, Assets<GlowMaterial>>,
    outlines: ResMut<'w, Assets<OutlineMaterial>>,
    looks: ResMut<'w, Assets<LookMaterial>>,
    images: ResMut<'w, Assets<Image>>,
}

/// What the x-ray is drawn through: the player's view mode, the kill cam and the player's camera.
#[derive(SystemParam)]
pub(super) struct Viewing<'w, 's> {
    sight: Option<Res<'w, SightMode>>,
    victim: Res<'w, Victim>,
    camera: Query<'w, 's, Entity, With<PlayerView>>,
}

/// Whether the x-ray draws this frame, and for which camera: the preference on, the subject
/// drawable in the third-person view, no kill cam run (this frame's, so after the kill cam's
/// drive). Inactive, or on another tank than the one dressed, everything is restored now; the mask
/// need is stated for [`look::own`], which runs next.
pub(super) fn drive(
    mut commands: Commands,
    xray: Res<XRay>,
    viewing: Viewing,
    subject: Subject,
    mut need: ResMut<MaskNeed>,
    mut state: ResMut<XRayState>,
    held: Held,
) {
    let third_person = viewing
        .sight
        .is_none_or(|mode| *mode == SightMode::ThirdPerson);
    let body = subject.body();
    let viewer = viewing.camera.single().ok();
    let active = xray.on && third_person && !viewing.victim.running() && body.is_some();
    let tank = body.map(|(entity, _)| entity);
    if !active
        || state
            .built
            .as_ref()
            .is_some_and(|built| Some(built.tank) != tank)
    {
        held.restore(&mut commands);
        state.built = None;
    }
    need.set_if_neq(MaskNeed(viewer.filter(|_| active)));
}

/// Dress the subject tank and draw the record over it, once its camera has a mask set; afterwards
/// keep it built: a mesh that lands late (a glb, a rung) is dressed as it appears, a new record or
/// a live state change rebuilds the record's elements, a new mask set (a resize) rebuilds the
/// whole x-ray.
#[expect(
    clippy::too_many_arguments,
    reason = "the x-ray's builder: the need and the masks, the clock, the record store, the blueprint, the subject, its own state, the asset server, the stores, the hierarchy and what it holds"
)]
pub(super) fn build(
    mut commands: Commands,
    need: Res<MaskNeed>,
    masks: Res<Masks>,
    time: Res<Time>,
    store: Option<Res<ShotExplanations>>,
    blueprint: Option<Res<TankBlueprint>>,
    subject: Subject,
    mut state: ResMut<XRayState>,
    asset_server: Res<AssetServer>,
    mut stores: Stores,
    scene: Hierarchy,
    held: Held,
) {
    let Some(viewer) = need.0 else {
        return;
    };
    let Some(set) = masks.of(viewer) else {
        return;
    };
    let (Some((tank, rig)), Some(life), Some(vehicle)) =
        (subject.body(), subject.life(), subject.vehicle())
    else {
        return;
    };
    let blueprint = blueprint.as_deref();
    let state = &mut *state;
    let Some(asset) = tank_asset(&mut state.tanks, &vehicle, blueprint) else {
        return;
    };
    let hull = ViewNode::resolve(scene.view_nodes.get(rig.hull).ok(), rig.hull);
    if state
        .built
        .as_ref()
        .is_some_and(|built| built.masks != *set || built.hull != hull)
    {
        // The restore lands with this frame's commands; the next frame dresses afresh, every
        // mesh undressed by then.
        held.restore(&mut commands);
        state.built = None;
        return;
    }
    let now = time.elapsed_secs_f64();
    let fresh = state.built.is_none();
    let built = state.built.get_or_insert_with(|| Built {
        tank,
        hull,
        masks: set.clone(),
        record: None,
        live: LiveState::default(),
        since: now,
        plan: Arc::new(Plan::bare(&TankShapes::default(), Vec::new())),
        atlas: None,
        tiers: HashMap::new(),
        mask: stores.looks.add(look::hull_mask(set)),
        settle: false,
        rescan: false,
    });

    // The tank's primitives, dressed as they stand or as they land.
    let landed = scene
        .added
        .iter()
        .any(|added| scene.parents.iter_ancestors(added).any(|a| a == tank));
    let mut dressed_now = false;
    if fresh || landed || built.rescan {
        let (dressed, skipped) = dress(
            &mut commands,
            tank,
            asset.spec(),
            built,
            &mut stores.looks,
            &scene,
            &held,
        );
        dressed_now = dressed;
        built.rescan = skipped;
    }

    // The record's elements, over the components as they are dressed.
    let latest = store
        .as_deref()
        .and_then(|store| subject.record(store, life));
    let record = latest.map(|(key, _)| key);
    let live = subject.live();
    let settle = std::mem::take(&mut built.settle);
    if !fresh && !settle && built.record == record && built.live == live {
        built.settle = dressed_now;
        return;
    }
    let same_record = built.record == record;
    if !same_record {
        // A record's first showing starts its clock; every later showing continues it.
        built.since = record.map_or(now, |key| *state.shown.entry(key).or_insert(now));
    }
    built.record = record;
    built.live = live;
    built.settle = dressed_now;
    for element in &held.elements {
        commands.entity(element).despawn();
    }
    let report = latest.map(|(_, stored)| &stored.report);
    let servo_deg = report
        .map(|report| report.servo_deg.clone())
        .unwrap_or_default();
    let mut plan = plan(report, &built.live, &asset.shapes(&servo_deg));
    if same_record {
        plan.share_maps(&built.plan);
    }
    let round_material = if plan
        .burst
        .as_ref()
        .is_some_and(|burst| burst.cone.is_some())
    {
        state
            .shell_material
            .get_or_insert_with(|| stage::shell_material(&asset_server))
            .clone()
    } else {
        Handle::default()
    };
    let components = component_meshes(
        &plan,
        held.dressed
            .iter()
            .map(|(entity, dressed, mesh)| (entity, dressed.names.as_slice(), mesh.0.clone())),
    );
    built.atlas = spawn_glass(
        &mut commands,
        &mut stores.meshes,
        &mut stores.glows,
        &mut stores.outlines,
        &plan,
        CornerKind::Victim,
        hull,
        None,
        &components,
        LookCtx {
            looks: &mut stores.looks,
            images: &mut stores.images,
            masks: set,
        },
        &round_material,
        (XRayItem, VisualScope::WORLD_EFFECT),
    );
    if plan.line.points.len() >= 2 {
        let shell = state
            .shell
            .get_or_insert_with(|| {
                asset_server.load(GltfAssetLabel::Scene(0).from_asset(SHELL_SCENE))
            })
            .clone();
        commands.spawn((
            WorldAssetRoot(shell),
            Transform::IDENTITY,
            Visibility::Hidden,
            VisualScope::WORLD_EFFECT,
            XRayShell,
            XRayItem,
            ChildOf(hull),
        ));
    }
    built.plan = Arc::new(plan);
}

/// The x-ray's round has landed: each of its meshes gets its mask copy against the x-ray's masks
/// as built. A new mask set rebuilds the x-ray, the round included, so the copies always hold the
/// current set. A global observer gated to the x-ray's shell, so the shell carries no observer
/// entity and the x-ray spawns nothing beyond its items and what hangs under them.
pub(super) fn round_landed(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    state: Res<XRayState>,
    shells: Query<(), With<XRayShell>>,
    children: Query<&Children>,
    meshes: Query<&Mesh3d>,
) {
    let Some(built) = &state.built else {
        return;
    };
    if !shells.contains(ready.entity) {
        return;
    }
    let round = children
        .iter_descendants(ready.entity)
        .filter_map(|entity| Some((entity, meshes.get(entity).ok()?.0.clone())));
    round_copies(&mut commands, round, &built.mask, ());
}

/// A new match: the store's keys start over, and so do the records shown. The x-ray as built
/// forgets its record too, so the next build re-keys the clock even for the same key, and
/// settles, so it redraws even with nothing left to draw.
pub(super) fn forget_shown(_new: On<NewMatch>, mut state: ResMut<XRayState>) {
    state.shown.clear();
    if let Some(built) = &mut state.built {
        built.record = None;
        built.settle = true;
    }
}

/// Dress every undressed render primitive under `root` in its tier's look, against `built`'s
/// masks, with a mask copy under it: whether any was, and whether one was left for the LOD
/// ladder's scan — a glTF primitive the ladder has not scanned yet (it binds the primitive's rungs
/// off the handle this would take), which the next frame looks at again; under
/// [`SourceDetailOnly`] no ladder scans, and every primitive is dressed as it lands. Left as they
/// are: the shadow ribbon (no camera draws it, and a copy of it in a mask would), a collision
/// proxy, and the x-ray's own meshes.
fn dress(
    commands: &mut Commands,
    root: Entity,
    spec: &crate::spec::TankSpec,
    built: &mut Built,
    looks: &mut Assets<LookMaterial>,
    scene: &Hierarchy,
    held: &Held,
) -> (bool, bool) {
    let mut any = false;
    let mut skipped = false;
    for entity in scene.children.iter_descendants(root) {
        let Ok((entity, mesh, material, range)) = held.undressed.get(entity) else {
            continue;
        };
        if scene.source_only.is_none()
            && scene.primitives.contains(entity)
            && !scene.scanned.contains(entity)
        {
            skipped = true;
            continue;
        }
        if scene
            .scopes
            .get(entity)
            .is_ok_and(|scope| *scope == VisualScope::SHADOW_PROXY)
        {
            continue;
        }
        if scene
            .parents
            .iter_ancestors(entity)
            .any(|ancestor| held.items.contains(ancestor))
        {
            continue;
        }
        let material_name = std::iter::once(entity)
            .chain(scene.parents.iter_ancestors(entity))
            .find_map(|node| scene.material_names.get(node).ok())
            .map(|name| name.0.as_str());
        let (names, tier, proxy) = view::chain_and_tier(
            entity,
            material_name,
            spec,
            &scene.parents,
            |node| node == root,
            &scene.names,
        );
        if proxy {
            continue;
        }
        let (handle, layer) = built
            .tiers
            .entry(tier)
            .or_insert_with(|| {
                let draw = look::hull(tier, &built.masks);
                (looks.add(draw.material), draw.layer)
            })
            .clone();
        commands
            .entity(entity)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert((
                MeshMaterial3d(handle),
                Dressed {
                    original: material.0.clone(),
                    names,
                },
            ));
        let mut copy = commands.spawn((
            mask_copy(mesh.0.clone(), built.mask.clone(), layer),
            XRayItem,
            MaskCopy { of: entity },
            ChildOf(entity),
        ));
        if let Some(range) = range {
            copy.insert(range.clone());
        }
        any = true;
    }
    (any, skipped)
}

/// Play the record's elements from the x-ray's clock: corner seconds since the record first
/// showed, held at the victim frame's hold beat.
#[expect(
    clippy::too_many_arguments,
    reason = "one query per disjoint x-ray element plus three material stores, the queries kept disjoint by their `Without` filters"
)]
pub(super) fn animate(
    time: Res<Time>,
    state: Res<XRayState>,
    knobs: Res<Knobs>,
    hulls: Query<&GlobalTransform>,
    mut glows: ResMut<Assets<GlowMaterial>>,
    mut outlines: ResMut<Assets<OutlineMaterial>>,
    mut looks: ResMut<Assets<LookMaterial>>,
    mut lines: Query<
        (
            &LineSeg,
            &MeshMaterial3d<GlowMaterial>,
            &mut Transform,
            &mut Visibility,
        ),
        (
            With<XRayItem>,
            Without<MarkItem>,
            Without<FieldVolume>,
            Without<XRayShell>,
        ),
    >,
    mut marks: Query<
        (&MarkItem, &MeshMaterial3d<GlowMaterial>, &mut Visibility),
        (
            With<XRayItem>,
            Without<LineSeg>,
            Without<FieldVolume>,
            Without<BoxTint>,
        ),
    >,
    mut volumes: Query<
        (&MeshMaterial3d<LookMaterial>, &mut Visibility),
        (
            With<XRayItem>,
            With<FieldVolume>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<BoxTint>,
        ),
    >,
    mut boxes: Query<
        (
            &BoxTint,
            Option<&MeshMaterial3d<OutlineMaterial>>,
            Option<&MeshMaterial3d<LookMaterial>>,
            &mut Visibility,
        ),
        (
            With<XRayItem>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<FieldVolume>,
        ),
    >,
    mut shell: Query<
        (&mut Transform, &mut Visibility),
        (
            With<XRayShell>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<FieldVolume>,
            Without<BoxTint>,
        ),
    >,
    mut pieces: Query<
        (&Piece, &mut Transform, &mut Visibility),
        (
            With<XRayItem>,
            Without<XRayShell>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<FieldVolume>,
            Without<BoxTint>,
        ),
    >,
) {
    let Some(built) = state.built.as_ref() else {
        return;
    };
    let hold_from = stage::pace(&built.plan, CornerKind::Victim).hold_from;
    let c = ((time.elapsed_secs_f64() - built.since) as f32).min(hold_from);
    let playing = Playing::new(&built.plan, CornerKind::Victim, c, knobs.reduce_motion);
    for (mut pose, mut visibility) in &mut shell {
        match playing.shell() {
            Some(want) => {
                if *pose != want {
                    *pose = want;
                }
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (piece, mut transform, mut visibility) in &mut pieces {
        playing.piece(piece, &mut transform, &mut visibility);
    }
    for (seg, material, mut transform, mut visibility) in &mut lines {
        playing.line(seg, material, &mut transform, &mut visibility, &mut glows);
    }
    for (mark, material, mut visibility) in &mut marks {
        playing.mark(mark, material, &mut visibility, &mut glows);
    }
    let hull = hulls.get(built.hull).ok();
    for (look, mut visibility) in &mut volumes {
        playing.fields(look, &mut visibility, hull, &mut looks);
    }
    if let Some(hull) = hull {
        playing.footprints(hull, &built.masks, &mut looks, built.atlas.as_ref());
    }
    for (tint, outline, look, mut visibility) in &mut boxes {
        playing.fill(
            tint,
            outline,
            look,
            &mut visibility,
            hull,
            &mut outlines,
            &mut looks,
        );
    }
}

/// Log each edge of the x-ray's drawing, on `Time<Real>` — the frame-cost stream's clock
/// (`src/frame_cost.rs`), so `scripts/perf/pencam_cost.py` can window that stream by the x-ray.
pub(super) fn log_presence(state: Res<XRayState>, time: Res<Time<Real>>, mut on: Local<bool>) {
    let now = state.built.is_some();
    if now == *on {
        return;
    }
    let t = time.elapsed_secs_f64();
    if now {
        debug!("pen cam: the x-ray is on at t={t:.3} s (Time<Real>)");
    } else {
        debug!("pen cam: the x-ray is off at t={t:.3} s (Time<Real>)");
    }
    *on = now;
}

/// A mask copy follows its source's mesh handle: a pooled shoe draws the rung its belt writes.
pub(super) fn follow_mesh(
    sources: Query<&Mesh3d, (Changed<Mesh3d>, With<Dressed>)>,
    mut copies: Query<(&MaskCopy, &mut Mesh3d), Without<Dressed>>,
) {
    for (copy, mut mesh) in &mut copies {
        if let Ok(source) = sources.get(copy.of)
            && mesh.0 != source.0
        {
            mesh.0 = source.0.clone();
        }
    }
}

/// The scripted preference: on from `from` to `to` app seconds, let go at `to`.
fn scripted(from: f32, to: f32) -> impl FnMut(Res<Time>, ResMut<XRay>, Local<bool>) {
    move |time: Res<Time>, mut xray: ResMut<XRay>, mut inside: Local<bool>| {
        let now = (from..to).contains(&time.elapsed_secs());
        if now != *inside {
            xray.on = now;
            *inside = now;
        }
    }
}

/// Dev-only, offline: `OVERMATCH_XRAY=<from>-<to>` holds the x-ray preference on from
/// `from` to `to` app seconds (`Time` as `Update` sees it) and lets it go at `to`, for captures.
/// Mounted only by the offline composition.
#[cfg(feature = "dev_tools")]
pub(crate) fn scripted_xray_plugin(app: &mut App) {
    let Ok(window) = std::env::var("OVERMATCH_XRAY") else {
        return;
    };
    let Some((from, to)) = window.split_once('-').and_then(|(from, to)| {
        Some((
            from.trim().parse::<f32>().ok()?,
            to.trim().parse::<f32>().ok()?,
        ))
    }) else {
        warn!("OVERMATCH_XRAY={window}: expected <from>-<to> seconds");
        return;
    };
    info!("pen cam: the x-ray is on from {from} s to {to} s (OVERMATCH_XRAY)");
    app.add_systems(Update, scripted(from, to).before(toggle));
}

/// One health-bearing volume of the subject tank as its replicated state stands.
#[derive(Clone, Debug, PartialEq)]
struct LiveBox {
    name: String,
    role: Role,
    current: u32,
    max: u32,
    dead: bool,
}

/// Every health-bearing volume of the subject tank, in the tank's volume order: its HP points
/// (`ComponentHealth`, which `NetCrew` writes on a network client) and whether its crew member is
/// knocked out (`Dead`).
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

    /// Each volume as the x-ray draws it, its shape from `shapes`, ending on its live state: its
    /// pool, its live points, lost at none left or with its crew member knocked out. One that the
    /// box of its name in `record` reaches starts from that box's points at the shot, never fewer
    /// than its live points, and rises to the end at that box's reach time; any other starts at
    /// the end. Invariant: `hp - lost_hp` is its live points, so its settled share is live's.
    fn boxes(&self, shapes: &TankShapes, record: &[BoxPlan]) -> Vec<BoxPlan> {
        self.0
            .iter()
            .map(|live| {
                let shape = shapes.get(&live.name);
                let reached = record
                    .iter()
                    .find(|b| b.name == live.name)
                    .and_then(|b| Some((b.hp, b.reached_at?)));
                let hp = reached.map_or(live.current, |(hp, _)| hp.max(live.current));
                BoxPlan {
                    name: live.name.clone(),
                    role: live.role,
                    center: shape.map(|shape| shape.center()),
                    heights: shape.map(|shape| {
                        let (lo, hi) = shape.bounds();
                        (lo.y, hi.y)
                    }),
                    count: live.max,
                    hp,
                    line: 0.0,
                    field: 0.0,
                    lost_hp: hp - live.current,
                    reached_at: reached.map(|(_, reach)| reach),
                    lost: live.dead || (live.max > 0 && live.current == 0),
                }
            })
            .collect()
    }
}

/// The x-ray's plan: `record`'s path and cones, or no round at all without one, over `live`'s
/// components, posed in `shapes`, each rising from its state at `record`'s shot where the record
/// reaches it ([`LiveState::boxes`]).
pub(super) fn plan(
    record: Option<&PenetrationReport>,
    live: &LiveState,
    shapes: &TankShapes,
) -> Plan {
    match record {
        Some(report) => {
            let mut plan = Plan::new(report, shapes);
            plan.kind = CornerKind::Victim;
            plan.boxes = live.boxes(shapes, &plan.boxes);
            plan
        }
        None => Plan::bare(shapes, live.boxes(shapes, &[])),
    }
}

/// Whether `stored` is a victim projection of a shot on `life`: the struck player's copy, never the
/// shooter's.
pub(super) fn struck(stored: &StoredExplanation, life: TankLife) -> bool {
    stored.explanation.audience != Audience::Shooter && stored.explanation.struck == life
}

/// The record the x-ray draws for the player's own `life`: of its victim projections whose impact
/// is no longer waiting, strictly the newest ([`newest`]).
pub(super) fn latest(
    store: &ShotExplanations,
    life: TankLife,
) -> Option<(ExplanationKey, &StoredExplanation)> {
    newest(store, life, |stored| struck(stored, life))
}

/// The record the x-ray draws for a struck tank's `life`: of the shooter projections of hits on it
/// whose impact is no longer waiting, strictly the newest ([`newest`]). The store holds the
/// player's records alone, so every shooter projection in it is of a shot the player fired.
pub(super) fn latest_shot(
    store: &ShotExplanations,
    life: TankLife,
) -> Option<(ExplanationKey, &StoredExplanation)> {
    newest(store, life, |stored| {
        stored.explanation.audience == Audience::Shooter
    })
}

/// Of the stored records of hits on `life` that `admits` and whose impact is no longer waiting,
/// strictly the newest by [`Recency`] (the encounter's first contact, then completion, identity
/// for ties), never by arrival.
fn newest(
    store: &ShotExplanations,
    life: TankLife,
    admits: impl Fn(&StoredExplanation) -> bool,
) -> Option<(ExplanationKey, &StoredExplanation)> {
    store
        .live(life)
        .filter(|stored| admits(stored) && stored.presence != ImpactPresence::Pending)
        .map(|stored| {
            let key = ExplanationKey::of(&stored.explanation);
            (Recency::of(key, &stored.explanation), key, stored)
        })
        .max_by_key(|(recency, _, _)| *recency)
        .map(|(_, key, stored)| (key, stored))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::damage_cam::pen_cam::demo::Library;
    use crate::damage_cam::pen_cam::timeline::Condition;
    use crate::damage_cam::shapes::TankAsset;

    /// Seventeen moments of a fill's rise, its start and its end among them.
    fn rises() -> impl Iterator<Item = f32> {
        (0..=16).map(|k| k as f32 / 16.0)
    }

    /// Every record of the demo's playlist, by stem, with its tank's shapes as posed at its shot.
    fn records() -> Vec<(&'static str, PenetrationReport, TankShapes)> {
        let mut assets = HashMap::<String, TankAsset>::new();
        Library::reports()
            .into_iter()
            .map(|(stem, report)| {
                let vehicle = report
                    .vehicle
                    .clone()
                    .expect("a recorded scenario names its vehicle");
                let shapes = assets
                    .entry(vehicle)
                    .or_insert_with_key(|vehicle| TankAsset::load(vehicle).unwrap())
                    .shapes(&report.servo_deg);
                (stem, report, shapes)
            })
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

    /// The live state of `record`'s components as its shot left them: their HP points at the shot
    /// less the points it took.
    fn after_shot(record: &PenetrationReport) -> LiveState {
        let mut live = standing(record);
        for b in &mut live.0 {
            let taken = record
                .doses
                .iter()
                .find(|dose| dose.volume == b.name)
                .map_or(0, |dose| dose.lost);
            b.current = b.current.saturating_sub(taken);
        }
        live
    }

    /// `live` with every component `points` HP points lower: points lost since the shot that no
    /// record tells (fixture data).
    fn lower(live: &LiveState, points: u32) -> LiveState {
        let mut lower = live.clone();
        for b in &mut lower.0 {
            b.current = b.current.saturating_sub(points);
        }
        lower
    }

    /// Where a live component's fill settles on the state scale: the share of its pool gone, out
    /// of action with no point left or its crew member knocked out.
    fn settled(live: &LiveBox) -> Option<Condition> {
        let share = if live.max == 0 {
            0.0
        } else {
            (1.0 - live.current as f32 / live.max as f32).clamp(0.0, 1.0)
        };
        Condition::of(live.dead || (live.max > 0 && live.current == 0), share)
    }

    /// `plan`'s components beside `live`'s, after checking the plan draws live's components and
    /// no other, in their order.
    fn beside<'a>(
        plan: &'a Plan,
        live: &'a LiveState,
    ) -> impl Iterator<Item = (&'a BoxPlan, &'a LiveBox)> {
        assert_eq!(
            plan.boxes
                .iter()
                .map(|b| b.name.as_str())
                .collect::<Vec<_>>(),
            live.0.iter().map(|l| l.name.as_str()).collect::<Vec<_>>(),
            "the x-ray draws live's components"
        );
        plan.boxes.iter().zip(&live.0)
    }

    /// Every component's fill settles on its live state whatever the record says — the record
    /// as shot; the same with no HP points at the shot, a point more taken from each component
    /// it doses and no consequences; no record — over live states as they stood at the shot, as
    /// the shot left them, a point lower since, at full, and with every crew member knocked out.
    /// Every viewer of the x-ray relies on it: the end of every rise is the tank as it stands.
    /// A mutant that ends on the record's after-shot share, or takes its out-of-action state
    /// from the record's consequences, breaks it.
    #[test]
    fn the_fill_settles_on_the_live_state() {
        let mut drawn = 0;
        for (stem, record, shapes) in records() {
            let mut rewritten = record.clone();
            for component in &mut rewritten.components {
                component.hp = 0;
            }
            for dose in &mut rewritten.doses {
                dose.lost += 1;
            }
            rewritten.consequences.clear();
            let shot = after_shot(&record);
            let mut full = standing(&record);
            for b in &mut full.0 {
                b.current = b.max;
            }
            let mut out = shot.clone();
            for b in out.0.iter_mut().filter(|b| b.role == Role::Crew) {
                b.dead = true;
            }
            for live in [standing(&record), lower(&shot, 1), shot, full, out] {
                for told in [Some(&record), Some(&rewritten), None] {
                    let plan = plan(told, &live, &shapes);
                    for (b, l) in beside(&plan, &live) {
                        assert_eq!(look::condition(b, 1.0), settled(l), "{stem} {}", b.name);
                        drawn += usize::from(settled(l).is_some());
                    }
                }
            }
        }
        assert!(drawn > 0, "a component with any of its pool gone");
    }

    /// A component the record reaches, and that had lost half its pool before the shot (fixture
    /// data: every pool doubled), starts where the record's own victim frame starts it — its
    /// state before the shot, at the record's reach time — and rises from there, never falling,
    /// to its live state. The victim relies on it: a fill at its end from the first frame shows a
    /// new shot's work before the picture reaches the component. A mutant that ignores the
    /// record, so nothing rises, breaks it.
    #[test]
    fn a_reached_component_rises_from_its_state_before_the_shot_to_live() {
        let mut rose = 0;
        for (stem, mut record, shapes) in records() {
            for component in &mut record.components {
                component.count *= 2;
            }
            let live = after_shot(&record);
            let shot = Plan::new(&record, &shapes);
            let plan = plan(Some(&record), &live, &shapes);
            for (b, l) in beside(&plan, &live) {
                let Some(r) = shot.boxes.iter().find(|r| r.name == b.name && r.struck()) else {
                    continue;
                };
                let name = format!("{stem} {}", b.name);
                assert!(r.reached_at.is_some(), "{name}: struck and never reached");
                assert_eq!(
                    b.reached_at, r.reached_at,
                    "{name}: the record's reach time"
                );
                let start = look::condition(b, 0.0);
                assert_eq!(start, look::condition(r, 0.0), "{name}: the record's start");
                assert_eq!(look::condition(b, 1.0), settled(l), "{name}: the live end");
                assert_ne!(start, settled(l), "{name}: nothing rises");
                let mut last = 0.0;
                for rise in rises() {
                    let fill = look::condition(b, rise).map_or(0.0, Condition::fill);
                    assert!(fill >= last, "{name}: falls at {rise}");
                    last = fill;
                }
                rose += 1;
            }
        }
        assert!(rose > 0, "a reached component the shot took HP from");
    }

    /// A record that has a component it reaches at fewer HP points at the shot than live holds
    /// now (fixture data: half the live points at the shot) never draws that component more
    /// damaged than live: its fill is the live state at every moment of the rise. The victim
    /// relies on it: a start more damaged than live would fall toward live, damage undone on
    /// screen. A mutant that drops the clamp of the start to live breaks it.
    #[test]
    fn the_start_is_never_more_damaged_than_live() {
        let mut clamped = 0;
        for (stem, record, shapes) in records() {
            let live = after_shot(&record);
            let mut older = record.clone();
            for component in &mut older.components {
                if let Some(l) = live.0.iter().find(|l| l.name == component.name) {
                    component.hp = l.current / 2;
                }
            }
            let plan = plan(Some(&older), &live, &shapes);
            for (b, l) in beside(&plan, &live) {
                for rise in rises() {
                    assert_eq!(
                        look::condition(b, rise),
                        settled(l),
                        "{stem} {} at {rise}",
                        b.name
                    );
                }
                clamped += usize::from(b.reached_at.is_some() && l.current / 2 < l.current);
            }
        }
        assert!(
            clamped > 0,
            "a reached component lower at the shot than live"
        );
    }

    /// A component the record does not reach holds its live state from the first frame through
    /// every moment of the rise: one the record does not name (fixture data: the record's first
    /// dosed component struck from it), one it names and never reached, and every one with no
    /// record — each a point lower since the shot, so each is drawn. The victim relies on it: a
    /// fill never waits on a reach the record does not have, nor starts from a state the record
    /// never stated. A mutant that rises from an untouched pool, or that takes the start of a
    /// named component the record never reached, breaks it.
    #[test]
    fn a_component_the_record_does_not_reach_holds_live_from_the_first_frame() {
        // Unnamed, named and never reached, no record.
        let mut held = [0; 3];
        for (stem, record, shapes) in records() {
            let Some(struck) = record.doses.first().map(|dose| dose.volume.clone()) else {
                continue;
            };
            let mut unnamed = record.clone();
            unnamed.components.retain(|c| c.name != struck);
            unnamed.doses.retain(|d| d.volume != struck);
            let live = lower(&after_shot(&record), 1);
            let reached = Plan::new(&unnamed, &shapes);
            for told in [Some(&unnamed), None] {
                let plan = plan(told, &live, &shapes);
                for (b, l) in beside(&plan, &live) {
                    let case = match told.and(reached.boxes.iter().find(|r| r.name == b.name)) {
                        Some(r) if r.reached_at.is_some() => continue,
                        Some(_) => 1,
                        None if told.is_some() => 0,
                        None => 2,
                    };
                    let name = format!("{stem} {}", b.name);
                    assert!(settled(l).is_some(), "{name}: fixture data: drawn");
                    assert_eq!(b.reached_at, None, "{name}: reached");
                    for rise in rises() {
                        assert_eq!(look::condition(b, rise), settled(l), "{name} at {rise}");
                    }
                    held[case] += 1;
                }
            }
        }
        assert!(held.iter().all(|n| *n > 0), "{held:?}");
    }
}
