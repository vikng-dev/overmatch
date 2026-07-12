//! View-layer impact VFX: a small emissive dust/spark puff at EVERY shell impact — the ship-facing
//! sibling of the dev-only `debug::spawn_impact_marker` (same `Impact` subscription, same
//! preloaded-assets + ring-buffer-eviction shape, but always on and animated instead of gizmo-gated
//! and static). It exists so the four non-tracer rounds of an MG belt cycle still READ at the
//! target: the rounds themselves stay invisible in flight (only every fifth gets a streak —
//! `ballistics::on_fire_shell`), but every round that lands puffs, so a burst visibly walks across
//! whatever it is hitting instead of one lone tracer arriving out of nowhere.
//!
//! Strictly view-only (ADR-0014): subscribes to the sim's [`Impact`] event and spawns short-lived
//! render entities that no sim system ever reads — safe on a predicting net client (the replica
//! still flies cosmetic shells and sparks `Impact`s; damage authority is untouched). Mounted by both
//! windowed client compositions (SP `ClientPlugin` and `NetClientPlugin`); the headless server and
//! the scripted harness never mount it.

use std::collections::VecDeque;

use bevy::color::Alpha;
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;

use crate::ballistics::Impact;

/// Puff lifetime (s): the whole expand-and-fade. Short — a spark of dust kicked off the surface,
/// not a smoke column; at the MG's cyclic rate anything longer starts stacking into fog.
const PUFF_LIFETIME: f32 = 0.35;
/// Sphere mesh radius (m) at scale 1 — the puff's spawn size.
const PUFF_RADIUS: f32 = 0.12;
/// Uniform scale the puff expands to by the end of its life (spawn size × this).
const PUFF_END_SCALE: f32 = 3.0;
/// Live-puff ring cap. Both MGs cycling (~25 impacts/s) at `PUFF_LIFETIME` keep ~9 alive, so the
/// cap is headroom for bursts/spall pileups, not steady state — the eviction is a leak bound, the
/// same job `debug::IMPACT_MARKER_CAP` does for the debug markers.
const PUFF_CAP: usize = 64;
/// The puff's emissive at birth (linear, above 1.0 so bloom catches it) — warm dust/spark, kept
/// well below the tracer streak's `LinearRgba::rgb(30, 12, 3)` so impacts read as secondary flashes,
/// not competing tracers.
const PUFF_EMISSIVE: LinearRgba = LinearRgba {
    red: 8.0,
    green: 5.0,
    blue: 3.0,
    alpha: 1.0,
};

pub fn plugin(app: &mut App) {
    app.init_resource::<PuffRing>()
        .add_systems(Startup, setup_puff_assets)
        // The ship impact puff: a view-side subscriber to `ballistics`' sim `Impact` event
        // (ADR-0014) — the same seam the debug marker and the sandbox subscribe to.
        .add_observer(spawn_impact_puff)
        .add_systems(Update, age_impact_puffs);
}

/// Preloaded puff view assets: the shared mesh handle plus the birth-state MATERIAL VALUE (not a
/// handle — each puff clones it into its own asset so the per-frame fade can mutate one puff
/// without fading every other live puff in lockstep).
#[derive(Resource)]
struct PuffAssets {
    mesh: Handle<Mesh>,
    material: StandardMaterial,
}

/// Live puffs in spawn order, oldest at the front — evicted past [`PUFF_CAP`], exactly the
/// `debug::ImpactMarkerRing` shape. Naturally-expired puffs leave stale entries behind; eviction
/// uses `try_despawn`, so a stale entry is a silent no-op.
#[derive(Resource, Default)]
struct PuffRing(VecDeque<Entity>);

/// A live impact puff's age (s); [`age_impact_puffs`] drives the expand/fade from it and despawns
/// the puff at [`PUFF_LIFETIME`].
#[derive(Component)]
struct ImpactPuff {
    age: f32,
}

fn setup_puff_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(PuffAssets {
        mesh: meshes.add(Sphere::new(PUFF_RADIUS)),
        // Same no-lit-contribution recipe as the tracer streak (black base, zero reflectance): the
        // emissive is the whole visual. `Blend` so the fade can sink it into the background —
        // Bevy scales the emissive contribution by the diffuse alpha under Blend, so the alpha fade
        // below fades the glow too, not just a black shell.
        material: StandardMaterial {
            base_color: Color::BLACK,
            reflectance: 0.0,
            emissive: PUFF_EMISSIVE,
            alpha_mode: AlphaMode::Blend,
            ..default()
        },
    });
}

/// Drop a puff at each shell impact — every round, tracer or not: the impact is what makes the
/// four invisible non-tracer rounds of the belt cycle read at the target.
fn spawn_impact_puff(
    impact: On<Impact>,
    assets: Res<PuffAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ring: ResMut<PuffRing>,
    mut commands: Commands,
) {
    // Per-puff material asset (see `PuffAssets::material`): the fade mutates it every frame. The
    // strong handle lives only on the puff entity, so despawning the puff frees the asset.
    let material = materials.add(assets.material.clone());
    let puff = commands
        .spawn((
            ImpactPuff { age: 0.0 },
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_translation(impact.position),
            // A glow puff neither casts nor receives shadow (same rule as the tracer streak).
            NotShadowCaster,
            NotShadowReceiver,
        ))
        .id();
    ring.0.push_back(puff);
    // Evict from the front until back under the cap (`try_despawn`: stale/already-expired entries
    // are silent no-ops).
    while ring.0.len() > PUFF_CAP {
        if let Some(old) = ring.0.pop_front() {
            commands.entity(old).try_despawn();
        }
    }
}

/// Expand and fade each live puff over its lifetime, despawning it at the end: scale runs
/// 1 → [`PUFF_END_SCALE`] while alpha and emissive run down to zero (the alpha also scales the
/// emissive under `Blend`, so the glow eases out rather than cutting).
fn age_impact_puffs(
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut puffs: Query<(
        Entity,
        &mut ImpactPuff,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut commands: Commands,
) {
    for (entity, mut puff, mut transform, material) in &mut puffs {
        puff.age += time.delta_secs();
        let t = puff.age / PUFF_LIFETIME;
        if t >= 1.0 {
            commands.entity(entity).despawn();
            continue;
        }
        transform.scale = Vec3::splat(1.0 + (PUFF_END_SCALE - 1.0) * t);
        if let Some(mut mat) = materials.get_mut(&material.0) {
            let fade = 1.0 - t;
            mat.base_color = mat.base_color.with_alpha(fade);
            mat.emissive = PUFF_EMISSIVE * fade;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal app carrying what the puff observer + ager read. Real `Assets` stores (initialized
    /// bare, no asset plugins) so the per-puff material clone and the fade mutation run for real.
    fn harness() -> App {
        let mut app = App::new();
        app.init_resource::<PuffRing>()
            .init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Time>()
            .add_observer(spawn_impact_puff)
            .add_systems(Update, age_impact_puffs);
        app.insert_resource(PuffAssets {
            mesh: Handle::default(),
            material: StandardMaterial {
                emissive: PUFF_EMISSIVE,
                ..default()
            },
        });
        app
    }

    fn puff_count(app: &mut App) -> usize {
        app.world_mut()
            .query_filtered::<Entity, With<ImpactPuff>>()
            .iter(app.world())
            .count()
    }

    #[test]
    fn every_impact_spawns_a_puff_and_ring_caps() {
        let mut app = harness();
        // Fire past the cap; the ring must hold at exactly the cap, oldest evicted — the same
        // leak bound the debug marker ring pins.
        for _ in 0..PUFF_CAP + 7 {
            app.world_mut().trigger(Impact {
                position: Vec3::ZERO,
            });
            app.world_mut().flush();
        }
        assert_eq!(puff_count(&mut app), PUFF_CAP);
        assert_eq!(app.world().resource::<PuffRing>().0.len(), PUFF_CAP);
    }

    #[test]
    fn puffs_expire_after_lifetime() {
        let mut app = harness();
        app.world_mut().trigger(Impact {
            position: Vec3::ZERO,
        });
        app.world_mut().flush();
        assert_eq!(puff_count(&mut app), 1);
        // Advance time past the lifetime; the ager must despawn the puff.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(PUFF_LIFETIME + 0.05));
        app.update();
        assert_eq!(puff_count(&mut app), 0, "an expired puff must despawn");
    }

    /// Mid-life, the puff must have EXPANDED (scale > 1) and FADED (alpha + emissive below birth) —
    /// the two halves of the read.
    #[test]
    fn puffs_expand_and_fade_over_life() {
        let mut app = harness();
        app.world_mut().trigger(Impact {
            position: Vec3::ZERO,
        });
        app.world_mut().flush();
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(PUFF_LIFETIME * 0.5));
        app.update();
        let world = app.world_mut();
        let mut q = world.query::<(&Transform, &MeshMaterial3d<StandardMaterial>, &ImpactPuff)>();
        let (transform, material, _) = q.single(world).expect("one live puff");
        assert!(
            transform.scale.x > 1.0,
            "mid-life puff must have expanded (scale {})",
            transform.scale.x
        );
        let handle = material.0.clone();
        let mat = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .expect("puff material asset");
        assert!(
            mat.base_color.alpha() < 1.0,
            "mid-life puff must have faded (alpha {})",
            mat.base_color.alpha()
        );
        assert!(
            mat.emissive.red < PUFF_EMISSIVE.red,
            "mid-life puff emissive must have dimmed (red {})",
            mat.emissive.red
        );
    }
}
