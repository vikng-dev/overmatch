//! Ballistics PRESENTATION: the shell scene, the MG tracer streak, and the visibility a hold draws
//! with.
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

use bevy::prelude::*;

use crate::render_policy::VisualScope;

use super::{Held, Projectile, ShellPath, TRACER_MAX_CALIBER, TracerRound};

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, setup_assets)
        // An `Add` observer, NOT an `Added<>` system: shells are born from TWO schedules
        // (`FixedUpdate` for local fire, `Update` for the net client's re-raise), and an ordinary
        // system in either one would leave a net-spawned round undressed for a frame — at 755 m/s
        // that is a ~12 m gap between the muzzle and the head of its streak.
        .add_observer(dress_projectile)
        .add_observer(hide_held_shell)
        .add_observer(show_released_shell);
}

/// Marks a projectile dressed with the main-gun shell scene — the DURABLE "this is a shell-class
/// round" interface `vfx` classifies on. (It used to classify on `WorldAssetRoot`, which is a
/// renderer implementation detail that happens to correlate.)
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

/// Preloaded shell scene, cloned per shot rather than loaded each time.
#[derive(Resource)]
struct ProjectileAssets {
    scene: Handle<WorldAsset>,
}

/// Preloaded tracer-streak assets (mesh + emissive material), built once so a tracer round clones
/// handles rather than rebuilding them per shot — the streak twin of [`ProjectileAssets`].
#[derive(Resource)]
struct TracerAssets {
    mesh: Handle<Mesh>,
    material: Handle<StandardMaterial>,
}

fn setup_assets(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Preload once; firing clones the handle rather than hitting the asset server per shot.
    commands.insert_resource(ProjectileAssets {
        scene: asset_server.load(GltfAssetLabel::Scene(0).from_asset("shell/shell.glb")),
    });
    // The tracer streak: a thin UNIT capsule authored along its local +Y. The per-shot child
    // transform ([`TracerStreak::drawn_transform`]) rotates that axis onto the shell's local −Z (its
    // travel axis — the projectile `Transform` is kept `look_to(velocity)` by
    // `integrate_projectiles`) and scales the capsule to the drawn length.
    let mesh = meshes.add(Capsule3d::new(0.018, 1.0));
    // The EMISSIVE IS THE WHOLE VISUAL: black base + zero reflectance kill every lit contribution,
    // so the streak renders exactly its emissive — which rides far above 1.0 in linear space, where
    // the HDR camera's `Bloom` (camera.rs) halos it and the tonemapper rolls the over-bright core to
    // white-hot for free. Do NOT set `unlit: true` here: StandardMaterial's unlit path outputs
    // `base_color` alone and IGNORES `emissive`, which rendered the old streak as a flat sRGB
    // "square sausage" that bloom never caught. Warm orange; magnitude tunes against bloom intensity.
    let material = materials.add(StandardMaterial {
        base_color: Color::BLACK,
        reflectance: 0.0,
        emissive: LinearRgba::rgb(30.0, 12.0, 3.0),
        ..default()
    });
    commands.insert_resource(TracerAssets { mesh, material });
}

/// The main-gun dressing, as a description: the scene root, the classification marker `vfx` reads,
/// and the visibility the round's hold state implies.
fn shell_dressing(assets: &ProjectileAssets, held: bool) -> impl Bundle {
    (
        ShellVisual,
        WorldAssetRoot(assets.scene.clone()),
        // Root visibility so the instantiated scene (and any effect child) inherits it.
        held_visibility(held),
    )
}

/// The MG tracer dressing, as a description: the streak child's own bundle.
fn tracer_dressing(assets: &TracerAssets, streak: TracerStreak, flown: f32) -> impl Bundle {
    (
        Mesh3d(assets.mesh.clone()),
        MeshMaterial3d(assets.material.clone()),
        // Seed clamped: an observer's round may be born after the per-frame maintainer has run.
        streak.drawn_transform(flown),
        // A light streak neither casts nor receives shadow — without this the sun dragged a long
        // capsule shadow across the terrain under every tracer. World geometry otherwise: it is a
        // child of a SHELL, never of a tank, so it is drawn in every view.
        VisualScope::WORLD_EFFECT,
        streak,
    )
}

/// What a shell in [`Held`] draws as: a hold is an INVISIBLE stop — no frozen round hanging on the
/// plate while the client waits for the authority's verdict.
fn held_visibility(held: bool) -> Visibility {
    if held {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    }
}

/// Dress a newly born projectile: main-gun scene, MG tracer streak, or nothing at all.
///
/// Bevy 0.19 writes the entire spawn bundle before it fires `Add`, so the round read here is the
/// complete one `on_fire_shell` authored — caliber, path and hold state included.
fn dress_projectile(
    add: On<Add, Projectile>,
    // `Without<WorldAssetRoot>` is the idempotency guard. Nothing in the tree re-inserts `Projectile`
    // today: `shooting::fire` suppresses `FireShell` on a replayed tick, and a projectile is not a
    // rollback-restored entity, so a replay raises no second birth. Insurance, not a live branch.
    shells: Query<
        (
            &Projectile,
            &Transform,
            &ShellPath,
            Has<Held>,
            Has<TracerRound>,
        ),
        Without<WorldAssetRoot>,
    >,
    assets: Res<ProjectileAssets>,
    tracer_assets: Res<TracerAssets>,
    mut commands: Commands,
) {
    let Ok((projectile, transform, path, held, tracer)) = shells.get(add.entity) else {
        return;
    };
    if projectile.caliber() >= TRACER_MAX_CALIBER {
        commands
            .entity(add.entity)
            .insert(shell_dressing(&assets, held));
    } else if tracer {
        // Scale with travel speed, with a floor for slow rounds.
        let streak = TracerStreak {
            nominal_len: (projectile.speed() * 0.018).max(2.0),
        };
        // Distance from the muzzle — `ShellPath` opens at the fire origin on both spawn paths, so a
        // net round caught up several ticks downrange seeds its streak at its true flown length.
        let flown = path
            .points
            .first()
            .map_or(0.0, |muzzle| transform.translation.distance(*muzzle));
        commands
            .entity(add.entity)
            // The root carries the visibility the streak child inherits.
            .insert(held_visibility(held))
            .with_child(tracer_dressing(&tracer_assets, streak, flown));
    }
    // A non-tracer MG round is dressed with nothing: it flies, raycasts and lands unseen.
}

/// A mid-flight hold hides whatever this round draws. The `With<Visibility>` filter IS the "is it
/// dressed" test — only [`dress_projectile`] puts one on a projectile, so an undrawn non-tracer MG
/// round never acquires render state just by holding.
fn hide_held_shell(
    add: On<Add, Held>,
    dressed: Query<(), (With<Projectile>, With<Visibility>)>,
    mut commands: Commands,
) {
    if dressed.contains(add.entity) {
        commands.entity(add.entity).insert(held_visibility(true));
    }
}

/// Releasing the hold — the authority's keyframe arrived, or it expired — shows the round again.
///
/// `try_insert`: a shell whose hold ends in a sanctioned TERMINAL is despawned in the same command
/// flush, and a despawn removes `Held` too. There is nothing left to show, and the write must be
/// silent rather than warn on a recycled id.
fn show_released_shell(
    remove: On<Remove, Held>,
    dressed: Query<(), (With<Projectile>, With<Visibility>)>,
    mut commands: Commands,
) {
    if dressed.contains(remove.entity) {
        commands
            .entity(remove.entity)
            .try_insert(held_visibility(false));
    }
}
