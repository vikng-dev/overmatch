//! The game's damage-model presentation: a round's body model and its belt's tracer streak, each
//! drawn from the round's authored visuals ([`crate::rounds::FlightEffects`]), never its calibre.
//!
//! Mounted by the client composition roots alone — `ClientPlugin`, `NetClientPlugin`, and the armor
//! sandbox (which mounts `ballistics` but NOT `vfx`, which is why this view half is owned here rather
//! than folded into `vfx`). The dedicated server composes [`super::sim_plugin`] and nothing else, so
//! it opens no shell glb, builds no mesh or material, and never pays visibility propagation for a
//! round in flight.
//!
//! The seam is ONE ENTITY, TWO COMPONENT SETS: the sim's projectile stays the root, and everything
//! here is a component or a child added to it. Nothing in this module writes simulation state — the
//! same structural split `geometry_lod`, `track` and `tank` already carry (ADR-0014, ADR-0035).

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::render_policy::VisualScope;

use super::{Projectile, ShellPath, TracerRound};
use crate::rounds::{FlightEffects, StreakEmission};

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, setup_assets)
        // An `Add` observer, NOT an `Added<>` system: shells are born from TWO schedules
        // (`FixedUpdate` for local fire, `Update` for the net client's re-raise), and an ordinary
        // system in either one would leave a net-spawned round undressed for a frame — at 755 m/s
        // that is a ~12 m gap between the muzzle and the head of its streak.
        .add_observer(dress_projectile);
}

/// Marks a projectile dressed with the shell scene. Composition tests inspect this identity;
/// flight emitters read their authored emission component independently of scene dressing.
#[derive(Component, Clone, Copy, Default)]
pub(crate) struct ShellVisual;

/// Tracer streak child of an MG round. The view clamps it to travel since the latest anchor.
#[derive(Component)]
pub struct TracerStreak {
    pub nominal_len: f32,
}

impl TracerStreak {
    /// Child transform for a streak that has travelled `flown` metres from its current anchor.
    ///
    /// Invariant: both spawn and view maintenance use this function, so the tail never precedes
    /// the muzzle or latest ricochet.
    pub(crate) fn drawn_transform(&self, flown: f32) -> Transform {
        let len = self.nominal_len.min(flown).max(0.0);
        Transform {
            translation: Vec3::Z * (len * 0.5),
            rotation: Quat::from_rotation_arc(Vec3::Y, Vec3::NEG_Z),
            scale: Vec3::new(1.0, len, 1.0),
        }
    }
}

/// Every round model's scene ([`crate::rounds::BodyModel`]), preloaded by scene path and cloned
/// per shot rather than loaded each time.
#[derive(Resource)]
struct ProjectileAssets {
    scenes: HashMap<&'static str, Handle<WorldAsset>>,
}

/// The streak meshes and materials built so far, one per authored radius and colour, so a tracer
/// clones handles rather than rebuilding them per shot — the streak twin of [`ProjectileAssets`].
#[derive(Resource, Default)]
struct TracerAssets {
    meshes: HashMap<u32, Handle<Mesh>>,
    materials: HashMap<[u32; 3], Handle<StandardMaterial>>,
}

fn setup_assets(mut commands: Commands, asset_server: Res<AssetServer>) {
    // Preload once; firing clones the handle rather than hitting the asset server per shot.
    let scenes = crate::rounds::BodyModel::shipped()
        .iter()
        .map(|model| {
            let scene = asset_server.load(GltfAssetLabel::Scene(0).from_asset(model.scene));
            (model.scene, scene)
        })
        .collect();
    commands.insert_resource(ProjectileAssets { scenes });
    commands.init_resource::<TracerAssets>();
}

impl TracerAssets {
    /// The streak's mesh and material: a thin capsule of the authored radius and UNIT length
    /// along its local +Y. The per-shot child transform ([`TracerStreak::drawn_transform`]) rotates
    /// that axis onto the shell's local −Z (its travel axis — the projectile `Transform` is kept
    /// `look_to(velocity)` by `integrate_projectiles`) and scales the capsule to the drawn length.
    fn of(
        &mut self,
        streak: &StreakEmission,
        meshes: &mut Assets<Mesh>,
        materials: &mut Assets<StandardMaterial>,
    ) -> (Handle<Mesh>, Handle<StandardMaterial>) {
        let mesh = self
            .meshes
            .entry(streak.radius_m.to_bits())
            .or_insert_with(|| meshes.add(Capsule3d::new(streak.radius_m, 1.0)))
            .clone();
        let [r, g, b] = streak.emissive;
        // The EMISSIVE IS THE WHOLE VISUAL: black base + zero reflectance kill every lit
        // contribution, so the streak renders exactly its emissive — which rides far above 1.0 in
        // linear space, where the HDR camera's `Bloom` (camera.rs) halos it and the tonemapper
        // rolls the over-bright core to white-hot for free. Do NOT set `unlit: true` here:
        // StandardMaterial's unlit path outputs `base_color` alone and IGNORES `emissive`.
        let material = self
            .materials
            .entry(streak.emissive.map(f32::to_bits))
            .or_insert_with(|| {
                materials.add(StandardMaterial {
                    base_color: Color::BLACK,
                    reflectance: 0.0,
                    emissive: LinearRgba::rgb(r, g, b),
                    ..default()
                })
            })
            .clone();
        (mesh, material)
    }
}

/// The body dressing, as a description: the scene root, the classification marker `vfx` reads,
/// and its inherited visibility.
fn shell_dressing(scene: Handle<WorldAsset>) -> impl Bundle {
    (
        ShellVisual,
        WorldAssetRoot(scene),
        // Root visibility so the instantiated scene (and any effect child) inherits it.
        Visibility::Inherited,
    )
}

/// The tracer dressing, as a description: the streak child's own bundle.
fn tracer_dressing(
    (mesh, material): (Handle<Mesh>, Handle<StandardMaterial>),
    streak: TracerStreak,
    flown: f32,
) -> impl Bundle {
    (
        Mesh3d(mesh),
        MeshMaterial3d(material),
        // Seed clamped: an observer's round may be born after the per-frame maintainer has run.
        streak.drawn_transform(flown),
        // A light streak neither casts nor receives shadow — without this the sun dragged a long
        // capsule shadow across the terrain under every tracer. World geometry otherwise: it is a
        // child of a SHELL, never of a tank, so it is drawn in every view.
        VisualScope::WORLD_EFFECT,
        streak,
    )
}

/// Dress a newly born projectile from its round's authored visuals ([`FlightEffects`]): the body's
/// model when the round draws one, and the streak when its belt marks it a tracer and the round
/// authors one. A round authoring neither flies, raycasts and lands unseen.
///
/// Bevy 0.19 writes the entire spawn bundle before it fires `Add`, so the round read here is the
/// complete one `on_fire_shell` authored — effects and path included.
fn dress_projectile(
    add: On<Add, Projectile>,
    shells: Query<(
        &Projectile,
        &FlightEffects,
        &Transform,
        &ShellPath,
        Has<TracerRound>,
        Has<WorldAssetRoot>,
    )>,
    assets: Res<ProjectileAssets>,
    mut tracer_assets: ResMut<TracerAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let (projectile, effects, transform, path, tracer, dressed) = shells
        .get(add.entity)
        .expect("a projectile spawns complete with its round's effects, pose and path");
    // The idempotency guard. Nothing in the tree re-inserts `Projectile` today: `shooting::fire`
    // suppresses `FireShell` on a replayed tick, and a projectile is not a rollback-restored
    // entity, so a replay raises no second birth. Insurance, not a live branch.
    if dressed {
        return;
    }
    if let Some(body) = effects.body {
        let scene = assets
            .scenes
            .get(body.scene)
            .expect("setup_assets preloads every embedded round model a round can name")
            .clone();
        commands.entity(add.entity).insert(shell_dressing(scene));
    }
    if tracer && let Some(look) = effects.streak {
        let streak = TracerStreak {
            nominal_len: (projectile.speed() * look.length_per_speed_s).max(look.min_length_m),
        };
        // Distance from the muzzle — `ShellPath` opens at the fire origin on both spawn paths, so a
        // net round caught up several ticks downrange seeds its streak at its true flown length.
        let flown = path
            .points
            .first()
            .map_or(0.0, |muzzle| transform.translation.distance(*muzzle));
        let handles = tracer_assets.of(&look, &mut meshes, &mut materials);
        commands
            .entity(add.entity)
            // The root carries the visibility the streak child inherits.
            .insert(Visibility::Inherited)
            .with_child(tracer_dressing(handles, streak, flown));
    }
}
