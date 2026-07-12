//! First-shot pipeline prewarm: kill the measured one-time ~1 s hitch on the first 88 fire by
//! making everything the first shot renders ALREADY WARM at startup.
//!
//! The hitch anatomy: the `shell.glb` HANDLE is preloaded at startup (`ballistics::setup_assets`),
//! so the asset bytes are in flight early — but nothing ever *instantiates or draws* the shell
//! until the first shot, which is when (a) the scene instantiates into entities and (b) wgpu
//! specializes + compiles the render pipelines for its mesh-layout × material permutation (on
//! Metal that shader compile is the classic ~1 s main-thread stall). The same lazy-compile applies
//! to every NEW vfx pipeline this slice adds (billboard Add, billboard Blend, trail ribbon, and
//! the impact puff's Blend `StandardMaterial`).
//!
//! The fix: at startup, spawn one of EACH — the real shell scene plus a quad/ribbon per vfx
//! material — far below the terrain, with `NoFrustumCulling` so the render world queues and
//! specializes them despite being nowhere near the frustum (a frustum-culled entity compiles
//! nothing, which is why "hide it under the map" alone would be a no-op). After a few seconds the
//! whole rig despawns; the pipelines stay cached for the session.
//!
//! View-only, mounted with the rest of `vfx` (windowed clients only) — the headless server never
//! sees any of this.

use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;

use super::ViewRng;
use super::billboard::{BillboardRing, BillboardSpec, VfxBillboardMaterial, spawn_billboard};
use super::ember::EmberAssets;
use super::impact::ImpactAssets;
use super::muzzle::MuzzleVfxAssets;
use super::trail::{TrailAssets, prewarm_ribbon_mesh};

/// Where the rig hides: far below the map (terrain sits near y = 0), still inside the camera's
/// default 1000 m far plane so depth math stays sane.
const PREWARM_POSITION: Vec3 = Vec3::new(0.0, -400.0, 0.0);
/// How long the rig lives (s). Generous: asset decode + scene instantiation + async pipeline
/// compilation all have to land inside it, and the rig costs nearly nothing while it does.
const PREWARM_SECS: f32 = 5.0;

/// A prewarm rig root: despawned (with its subtree) when the timer runs out.
#[derive(Component)]
pub(super) struct PrewarmRig {
    timer: Timer,
}

impl PrewarmRig {
    fn new() -> Self {
        Self {
            timer: Timer::from_seconds(PREWARM_SECS, TimerMode::Once),
        }
    }
}

/// Spawn the warm-up set: the shell scene + one billboard per blend mode + one trail ribbon + one
/// impact puff. Ordered after the other vfx Startup setups (see `vfx::plugin`) so their preloaded
/// assets exist.
pub(super) fn spawn_prewarm_rig(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    muzzle: Res<MuzzleVfxAssets>,
    trail: Res<TrailAssets>,
    impact: Res<ImpactAssets>,
    ember: Res<EmberAssets>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut billboard_materials: ResMut<Assets<VfxBillboardMaterial>>,
    mut standard_materials: ResMut<Assets<StandardMaterial>>,
    mut rng: ResMut<ViewRng>,
    mut ring: ResMut<BillboardRing>,
) {
    // The 88 shell scene — the same asset path `ballistics::setup_assets` preloads, so this rides
    // the identical (already started) load and only ADDS the instantiate + first-draw that used to
    // happen on the first shot. Its glTF meshes get `NoFrustumCulling` as they appear
    // (`tag_prewarm_meshes`); the root can't carry it for them.
    commands.spawn((
        PrewarmRig::new(),
        WorldAssetRoot(asset_server.load(GltfAssetLabel::Scene(0).from_asset("shell/shell.glb"))),
        Transform::from_translation(PREWARM_POSITION),
    ));

    // One billboard per material permutation the muzzle dressing draws: additive flash (the real
    // core atlas — a default handle would never finish preparing and the pipeline would never
    // compile) and alpha-blend smoke. Spawned through the real spawner so the entity shape matches
    // a live one, then re-tagged as rig members with culling disabled.
    for material in [
        muzzle.flash_material(muzzle.core_atlas.clone(), 2.0),
        muzzle.smoke_material(),
        // The impact sparks' template: same Add-blend quad pipeline as the flash (which also covers
        // the additive ping), but its own LUT — warming it readies the spark bind group too. The
        // impact dust billow rides the smoke's Blend pipeline, already warmed above.
        impact.spark_material(),
    ] {
        let billboard = spawn_billboard(
            &mut commands,
            &mut billboard_materials,
            &mut ring,
            muzzle.quad.clone(),
            BillboardSpec {
                material,
                // Long enough that the ager can't kill it before the pipeline compiles; the rig
                // timer (or the billboard ring) reaps it regardless.
                lifetime: PREWARM_SECS * 2.0,
                origin: PREWARM_POSITION,
                drift: Vec3::ZERO,
                frames: 4,
                start_frame: rng.range(0.0, 4.0),
                frame_rate: 1.0,
                start_size: 1.0,
                end_size: 1.0,
                aspect: Vec3::ONE,
                roll: 0.0,
                spin: 0.0,
                erosion_end: 0.5,
                rotation: None,
            },
        );
        commands
            .entity(billboard)
            .insert((PrewarmRig::new(), NoFrustumCulling));
    }

    // One real trail ribbon (its own mesh layout — position/normal/uv/color — is its own pipeline
    // permutation) and one 88 ember clone (Blend emissive `StandardMaterial` over the ember sphere —
    // the same pipeline the old impact puff warmed, now serving the shell-base ember).
    commands.spawn((
        PrewarmRig::new(),
        Mesh3d(meshes.add(prewarm_ribbon_mesh())),
        MeshMaterial3d(trail.material.clone()),
        Transform::from_translation(PREWARM_POSITION),
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
    ));
    commands.spawn((
        PrewarmRig::new(),
        Mesh3d(ember.mesh.clone()),
        MeshMaterial3d(standard_materials.add(ember.material.clone())),
        Transform::from_translation(PREWARM_POSITION),
        NoFrustumCulling,
        NotShadowCaster,
        NotShadowReceiver,
    ));
}

/// As the warm-up shell scene instantiates, stamp `NoFrustumCulling` (+ shadow opt-outs) onto its
/// mesh entities — the piece that makes an under-the-map scene actually reach pipeline
/// specialization. Idles at one empty-query check once the rigs are gone.
pub(super) fn tag_prewarm_meshes(
    rigs: Query<(), With<PrewarmRig>>,
    untagged: Query<Entity, (With<Mesh3d>, Without<NoFrustumCulling>)>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    if rigs.is_empty() {
        return;
    }
    for entity in &untagged {
        // Walk up to see whether this mesh belongs to a prewarm rig (the shell scene is 2–3 nodes
        // deep; live gameplay meshes stop at their own non-rig root).
        let mut node = entity;
        loop {
            if rigs.contains(node) {
                commands.entity(entity).insert((
                    NoFrustumCulling,
                    NotShadowCaster,
                    NotShadowReceiver,
                ));
                break;
            }
            match parents.get(node) {
                Ok(parent) => node = parent.parent(),
                Err(_) => break,
            }
        }
    }
}

/// Reap each rig (and its subtree) when its timer expires — the pipelines it warmed stay cached
/// for the session.
pub(super) fn expire_prewarm(
    time: Res<Time>,
    mut rigs: Query<(Entity, &mut PrewarmRig)>,
    mut commands: Commands,
) {
    for (entity, mut rig) in &mut rigs {
        if rig.timer.tick(time.delta()).just_finished() {
            commands.entity(entity).try_despawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rig mechanics, headless: scene meshes that appear UNDER a rig get culling disabled
    /// (that is what makes the prewarm actually compile pipelines), unrelated meshes stay
    /// untouched, and the whole rig reaps itself after its timer.
    #[test]
    fn rig_tags_its_meshes_and_expires() {
        let mut app = App::new();
        app.init_resource::<Time>()
            .add_systems(Update, (tag_prewarm_meshes, expire_prewarm));

        let root = app
            .world_mut()
            .spawn((
                PrewarmRig::new(),
                Transform::default(),
                Visibility::default(),
            ))
            .id();
        let scene_mesh = app
            .world_mut()
            .spawn((Mesh3d(Handle::default()), ChildOf(root)))
            .id();
        let unrelated = app.world_mut().spawn(Mesh3d(Handle::default())).id();
        app.update();

        assert!(
            app.world().get::<NoFrustumCulling>(scene_mesh).is_some(),
            "a rig-descendant mesh must get NoFrustumCulling (else the prewarm compiles nothing)"
        );
        assert!(
            app.world().get::<NoFrustumCulling>(unrelated).is_none(),
            "meshes outside the rig must keep normal culling"
        );

        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(PREWARM_SECS + 0.1));
        app.update();
        assert!(
            app.world().get_entity(root).is_err(),
            "the rig root must reap itself"
        );
        assert!(
            app.world().get_entity(scene_mesh).is_err(),
            "the rig subtree despawns with it"
        );
    }
}
