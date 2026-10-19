//! The own glass: the player's own tank in the glass look on the battlefield, the latest shot that
//! struck it drawn over it, its key ([`OwnGlass`], X) a preference kept across lives. Its own
//! presentation beside the corner slot: no corner, no stage copy, no exhibit. The look goes on the
//! tank's REAL render primitives — each mesh's `StandardMaterial` handle set aside ([`Dressed`])
//! and replaced by its tier's [`LookMaterial`], a mask copy spawned under it — and the record's
//! elements hang under the live hull and component meshes ([`spawn_glass`]), so the pose, the
//! servos, the certified LOD and the pooled shoes follow for free; nothing is hidden and nothing
//! is copied. One mask set is owned for the player's camera ([`MaskNeed`]). While on, the tank
//! casts no shadow (`LookMaterial` casts none).
//!
//! * **When it draws.** The preference on, the player's tank live (a life, not knocked out, its
//!   rig bound), the third-person view and no kill cam run ([`drive`]). Every deactivation —
//!   the key, a knockout or the run it starts, the gunner's optic, a new tank, a disconnect —
//!   restores every handle and removes every tag and child at once ([`restore`]); the preference
//!   stays, so the glass returns on the next live tank.
//! * **The tint reads live state only.** Each component's fill is its live HP points over its pool
//!   ([`LiveState::boxes`]), never the record's: the record supplies the path and the cones, and a
//!   component's dose or HP at the shot never reaches the glass. An unstruck component (no point
//!   lost) is the base layout, untinted.
//! * **The latest rule.** Of the victim records of the player's current life whose impact is no
//!   longer waiting, strictly the newest by recency ([`latest`]; a clean ricochet replaces an
//!   earlier penetration — a bounce is informative too). With none, the live tint alone.
//! * **The clock.** The record plays from the moment it first shows (the toggle, or a newer
//!   record), as the victim frame plays it, and holds at its hold beat; the tint follows the live
//!   state in the same showing.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::camera::visibility::VisibilityRange;
use bevy::ecs::query::QueryData;
use bevy::ecs::system::SystemParam;
use bevy::gltf::{GltfMaterialName, GltfMeshName};
use bevy::prelude::*;
use bevy::window::{CursorOptions, PrimaryWindow};
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

use super::super::Knobs;
use super::super::look::{self, LookMaterial, MaskLayer, MaskNeed, MaskSet, Masks};
use super::super::material::{GlowMaterial, OutlineMaterial};
use super::super::plan::{BoxPlan, Plan};
use super::super::stage::{
    BoxTint, ConeShell, LineSeg, LookCtx, MarkItem, Playing, SHELL_SCENE, component_meshes,
    mask_copy, spawn_glass,
};
use super::super::timeline::CornerKind;
use super::super::view::{self, Tier};
use super::killcam::Victim;
use super::{Player, Recency, tank_asset};
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
use crate::geometry_lod::GeometryLodScanned;
use crate::render_policy::VisualScope;
use crate::sight::SightMode;
use crate::tank::{Controlled, ViewNode};
use crate::view::PlayerView;

/// The player's own-glass preference, which [`Bindings::own_glass`] toggles: kept across lives,
/// respawn, the map and a disconnect, so the glass returns on the next live tank.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct OwnGlass {
    pub(crate) on: bool,
}

/// Flip [`OwnGlass`] on the key, while the cursor is captured (a menu or a pause takes the key)
/// and the player's tank is live: knocked out, the death flow owns the device and a press changes
/// nothing, so a kill cam run never flips it.
pub(super) fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    bindings: Option<Res<Bindings>>,
    cursor: Query<&CursorOptions, With<PrimaryWindow>>,
    live: Query<(), (With<Controlled>, Without<TankKnockedOut>)>,
    mut glass: ResMut<OwnGlass>,
) {
    let pressed = bindings.is_some_and(|bindings| bindings.own_glass.just_pressed(&keys, &mouse));
    if pressed && !live.is_empty() && crate::state::cursor_locked(cursor) {
        glass.on = !glass.on;
    }
}

/// An entity the glass spawned — a mask copy, a record element, the shell — despawned whole on
/// restore. Never under another: a copy hangs under a dressed mesh, an element under the live
/// hull or a dressed mesh.
#[derive(Component, Clone, Copy)]
pub(super) struct GlassItem;

/// A live render primitive wearing the look: the material it wore, restored exactly, and the names
/// of its node chain up to the tank's root, nearest first, by which the record's components find
/// their meshes.
#[derive(Component)]
pub(super) struct Dressed {
    original: Handle<StandardMaterial>,
    names: Vec<String>,
}

/// The glass's mask copy of a dressed mesh, drawn for the mask cameras alone: it follows the
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

/// The glass's shell model, the round resting where the record ends.
#[derive(Component)]
pub(super) struct GlassShell;

/// What the glass stands on between frames.
#[derive(Resource, Default)]
pub(super) struct Glass {
    built: Option<Built>,
    /// Each tank file's asset, read once.
    tanks: HashMap<String, Arc<TankAsset>>,
    /// The shell model, loaded once the glass first draws a record.
    shell: Option<Handle<WorldAsset>>,
}

impl Glass {
    /// The plan the glass draws, while built.
    #[cfg(test)]
    pub(super) fn plan(&self) -> Option<&Plan> {
        self.built.as_ref().map(|built| &*built.plan)
    }

    /// App seconds the record on show first showed, while built.
    #[cfg(test)]
    pub(super) fn since(&self) -> Option<f64> {
        self.built.as_ref().map(|built| built.since)
    }
}

/// The glass as built: what it was built from, and the materials it dressed the tank in.
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
    /// One material per tier the tank's meshes wear, and the layer each is masked in.
    tiers: HashMap<Tier, (Handle<LookMaterial>, MaskLayer)>,
    /// The masks' material, on every mask copy.
    mask: Handle<LookMaterial>,
    /// Meshes were dressed this frame: the record's elements are built again next frame, once
    /// their tints can hang under them.
    settle: bool,
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
    scopes: Query<'w, 's, &'static VisualScope>,
    view_nodes: Query<'w, 's, &'static ViewNode>,
    added: Query<'w, 's, Entity, Added<Mesh3d>>,
}

/// What the glass holds in the world: the dressed meshes, its items, and what is still to dress.
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
        (Without<Dressed>, Without<GlassItem>),
    >,
    dressed: Query<'w, 's, (Entity, &'static Dressed, &'static Mesh3d)>,
    items: Query<'w, 's, Entity, With<GlassItem>>,
    elements: Query<'w, 's, Entity, (With<GlassItem>, Without<MaskCopy>)>,
}

impl Held<'_, '_> {
    /// Put every dressed mesh back in the material it wore and despawn every item of the glass.
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

/// The glass's material stores.
#[derive(SystemParam)]
pub(super) struct Stores<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    glows: ResMut<'w, Assets<GlowMaterial>>,
    outlines: ResMut<'w, Assets<OutlineMaterial>>,
    looks: ResMut<'w, Assets<LookMaterial>>,
}

/// What the glass is drawn through: the player's view mode, the kill cam and the player's camera.
#[derive(SystemParam)]
pub(super) struct Viewing<'w, 's> {
    sight: Option<Res<'w, SightMode>>,
    victim: Res<'w, Victim>,
    camera: Query<'w, 's, Entity, With<PlayerView>>,
}

/// Whether the glass draws this frame, and for which camera: the preference on, the tank live in
/// the third-person view, no kill cam run (this frame's, so after the kill cam's drive). Inactive,
/// or on another tank than the one dressed, everything is restored now; the mask need is stated
/// for [`look::own`], which runs next.
pub(super) fn drive(
    mut commands: Commands,
    glass: Res<OwnGlass>,
    viewing: Viewing,
    player: Player,
    mut need: ResMut<MaskNeed>,
    mut state: ResMut<Glass>,
    held: Held,
) {
    let third_person = viewing
        .sight
        .is_none_or(|mode| *mode == SightMode::ThirdPerson);
    let body = player.body();
    let viewer = viewing.camera.single().ok();
    let active = glass.on && third_person && !viewing.victim.running() && body.is_some();
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

/// Dress the live tank and draw the record over it, once its camera has a mask set; afterwards
/// keep it built: a mesh that lands late (a glb, a rung) is dressed as it appears, a new record or
/// a live state change rebuilds the record's elements, a new mask set (a resize) rebuilds the
/// whole glass.
#[expect(
    clippy::too_many_arguments,
    reason = "the glass's builder: the need and the masks, the clock, the record store, the blueprint, the player, its own state, the asset server, the stores, the hierarchy and what it holds"
)]
pub(super) fn build(
    mut commands: Commands,
    need: Res<MaskNeed>,
    masks: Res<Masks>,
    time: Res<Time>,
    store: Option<Res<ShotExplanations>>,
    blueprint: Option<Res<TankBlueprint>>,
    player: Player,
    mut state: ResMut<Glass>,
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
        (player.body(), player.life(), player.vehicle())
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
        held.restore(&mut commands);
        state.built = None;
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
        tiers: HashMap::new(),
        mask: stores.looks.add(look::hull_mask(set)),
        settle: false,
    });

    // The tank's primitives, dressed as they stand or as they land.
    let landed = scene
        .added
        .iter()
        .any(|added| scene.parents.iter_ancestors(added).any(|a| a == tank));
    let mut dressed_now = false;
    if fresh || landed {
        dressed_now = dress(
            &mut commands,
            tank,
            asset.spec(),
            built,
            &mut stores.looks,
            &scene,
            &held,
        );
    }

    // The record's elements, over the components as they are dressed.
    let latest = store.as_deref().and_then(|store| latest(store, life));
    let record = latest.map(|(key, _)| key);
    let live = player.live();
    let settle = std::mem::take(&mut built.settle);
    if !fresh && !settle && built.record == record && built.live == live {
        built.settle = dressed_now;
        return;
    }
    if built.record != record {
        built.since = now;
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
    let plan = plan(report, &built.live, &asset.shapes(&servo_deg));
    let components = component_meshes(
        &plan,
        held.dressed
            .iter()
            .map(|(entity, dressed, mesh)| (entity, dressed.names.as_slice(), mesh.0.clone())),
    );
    spawn_glass(
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
            masks: set,
        },
        (GlassItem, VisualScope::WORLD_EFFECT),
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
            GlassShell,
            GlassItem,
            ChildOf(hull),
        ));
    }
    built.plan = Arc::new(plan);
}

/// Dress every undressed render primitive under `root` in its tier's look, against `built`'s
/// masks, with a mask copy under it; whether any was. Left as it is: a glTF primitive the LOD
/// ladder has not scanned yet (it binds the primitive's rungs off the handle this would take),
/// the shadow ribbon (no camera draws it, and a copy of it in a mask would), a collision proxy,
/// and the glass's own meshes.
fn dress(
    commands: &mut Commands,
    root: Entity,
    spec: &crate::spec::TankSpec,
    built: &mut Built,
    looks: &mut Assets<LookMaterial>,
    scene: &Hierarchy,
    held: &Held,
) -> bool {
    let mut any = false;
    for entity in scene.children.iter_descendants(root) {
        let Ok((entity, mesh, material, range)) = held.undressed.get(entity) else {
            continue;
        };
        if scene.primitives.contains(entity) && !scene.scanned.contains(entity) {
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
                (looks.add(draw.material), draw.mask)
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
            GlassItem,
            MaskCopy { of: entity },
            ChildOf(entity),
        ));
        if let Some(range) = range {
            copy.insert(range.clone());
        }
        any = true;
    }
    any
}

/// Play the record's elements from the glass's clock: corner seconds since the record first
/// showed, held at the victim frame's hold beat.
#[expect(
    clippy::too_many_arguments,
    reason = "one query per disjoint glass element plus three material stores, the queries kept disjoint by their `Without` filters"
)]
pub(super) fn animate(
    time: Res<Time>,
    state: Res<Glass>,
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
            With<GlassItem>,
            Without<MarkItem>,
            Without<ConeShell>,
            Without<GlassShell>,
        ),
    >,
    mut marks: Query<
        (&MarkItem, &MeshMaterial3d<GlowMaterial>, &mut Visibility),
        (
            With<GlassItem>,
            Without<LineSeg>,
            Without<ConeShell>,
            Without<BoxTint>,
        ),
    >,
    mut shells: Query<
        (
            &ConeShell,
            Option<&MeshMaterial3d<GlowMaterial>>,
            Option<&MeshMaterial3d<LookMaterial>>,
            &mut Visibility,
        ),
        (
            With<GlassItem>,
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
            With<GlassItem>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<ConeShell>,
        ),
    >,
    mut shell: Query<
        (&mut Transform, &mut Visibility),
        (
            With<GlassShell>,
            Without<LineSeg>,
            Without<MarkItem>,
            Without<ConeShell>,
            Without<BoxTint>,
        ),
    >,
) {
    let Some(built) = state.built.as_ref() else {
        return;
    };
    let c = ((time.elapsed_secs_f64() - built.since) as f32).min(CornerKind::Victim.hold_from());
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
    for (seg, material, mut transform, mut visibility) in &mut lines {
        playing.line(seg, material, &mut transform, &mut visibility, &mut glows);
    }
    for (mark, material, mut visibility) in &mut marks {
        playing.mark(mark, material, &mut visibility, &mut glows);
    }
    for (cone, glow, look, mut visibility) in &mut shells {
        playing.cone(cone, glow, look, &mut visibility, &mut glows, &mut looks);
    }
    let hull = hulls.get(built.hull).ok();
    if let Some(hull) = hull {
        playing.footprints(hull, &built.masks, &mut looks);
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

/// Log each edge of the glass's drawing, on `Time<Real>` — the frame-cost stream's clock
/// (`src/frame_cost.rs`), so `scripts/perf/pencam_cost.py` can window that stream by the glass.
pub(super) fn log_presence(state: Res<Glass>, time: Res<Time<Real>>, mut on: Local<bool>) {
    let now = state.built.is_some();
    if now == *on {
        return;
    }
    let t = time.elapsed_secs_f64();
    if now {
        debug!("pen cam: the own glass is on at t={t:.3} s (Time<Real>)");
    } else {
        debug!("pen cam: the own glass is off at t={t:.3} s (Time<Real>)");
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
fn scripted(from: f32, to: f32) -> impl FnMut(Res<Time>, ResMut<OwnGlass>, Local<bool>) {
    move |time: Res<Time>, mut glass: ResMut<OwnGlass>, mut inside: Local<bool>| {
        let now = (from..to).contains(&time.elapsed_secs());
        if now != *inside {
            glass.on = now;
            *inside = now;
        }
    }
}

/// Dev-only, offline: `OVERMATCH_OWN_GLASS=<from>-<to>` holds the own-glass preference on from
/// `from` to `to` app seconds (`Time` as `Update` sees it) and lets it go at `to`, for captures.
/// Mounted only by the offline composition.
#[cfg(feature = "dev_tools")]
pub(crate) fn scripted_glass_plugin(app: &mut App) {
    let Ok(window) = std::env::var("OVERMATCH_OWN_GLASS") else {
        return;
    };
    let Some((from, to)) = window.split_once('-').and_then(|(from, to)| {
        Some((
            from.trim().parse::<f32>().ok()?,
            to.trim().parse::<f32>().ok()?,
        ))
    }) else {
        warn!("OVERMATCH_OWN_GLASS={window}: expected <from>-<to> seconds");
        return;
    };
    info!("pen cam: the own glass is on from {from} s to {to} s (OVERMATCH_OWN_GLASS)");
    app.add_systems(Update, scripted(from, to).before(toggle));
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

/// Whether `stored` is a victim projection of a shot on `life`: the struck player's copy, never the
/// shooter's.
pub(super) fn struck(stored: &StoredExplanation, life: TankLife) -> bool {
    stored.explanation.audience != Audience::Shooter && stored.explanation.struck == life
}

/// The record the glass draws for `life`: of its victim projections whose impact is no longer
/// waiting, strictly the newest by [`Recency`] (the encounter's first contact, then completion,
/// identity for ties), never by arrival.
pub(super) fn latest(
    store: &ShotExplanations,
    life: TankLife,
) -> Option<(ExplanationKey, &StoredExplanation)> {
    store
        .live(life)
        .filter(|stored| struck(stored, life) && stored.presence != ImpactPresence::Pending)
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
        let mut killed = live;
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

    /// The scripted preference holds on inside its window and lets go at its end, and only at
    /// its edges: a press inside the window stands until the window ends.
    #[test]
    fn the_scripted_preference_holds_for_its_window() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .init_resource::<OwnGlass>()
            .add_systems(Update, scripted(1.0, 3.0));
        let on = |app: &App| app.world().resource::<OwnGlass>().on;
        let at = |app: &mut App, seconds: f32| {
            let now = app.world().resource::<Time>().elapsed_secs();
            app.world_mut()
                .resource_mut::<Time>()
                .advance_by(std::time::Duration::from_secs_f32(seconds - now));
            app.update();
        };
        at(&mut app, 0.5);
        assert!(!on(&app));
        at(&mut app, 1.5);
        assert!(on(&app), "on inside the window");
        app.world_mut().resource_mut::<OwnGlass>().on = false;
        at(&mut app, 2.0);
        assert!(!on(&app), "the edge alone writes it");
        at(&mut app, 3.5);
        assert!(!on(&app), "off past the window");
    }
}
