//! The 88's tracer ember (survey/historical read): a small, GLOWING red-orange emissive point at the
//! shell's BASE that burns for ~2 s then fades — the Pzgr.39's base tracer (APCBC-HE-T, ~13 g of
//! tracer composition, ~2 s burn to ~1500 m), the gunner's fall-of-shot read at range. It is
//! deliberately a point at the base, NEVER a whole-shell glow (the War-Thunder "glowing
//! telephone-pole" failure mode), and quieter/steadier than the MG tracer streaks: the trail is the
//! shell's PATH history, the ember is its CURRENT position, readable at 1500 m+ and at the moment of
//! impact.
//!
//! It rides the 88 shell as a child (attached by the same `ShellPath + WorldAssetRoot` signature the
//! smoke trail uses — [`super::trail`]), so it follows the round for free and despawns with it at
//! impact. View-only (ADR-0014), client-mounted; the headless server never mounts `vfx`.
//!
//! Hardcoded on the 88's single ammunition nature for now. Real ammunition differs — HE (Sprgr.) is
//! untraced, tracer color and burn vary by round — so this is the seed of a future per-ammo
//! `tracer: Option<TracerSpec>` (color / intensity / burn) carried on the shell's nature, at which
//! point the constants below become that spec's default.

use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;

use crate::ballistics::ShellPath;

/// The ember's emissive at full burn (linear) — well up into the bloom-catching band so the 88's
/// point reads as ONE deliberate glowing ember with a small halo, not the pre-tuning non-glowing dot.
/// Still kept clearly under the MG tracer streak's `LinearRgba::rgb(30, 12, 3)` so it never competes
/// with a streak: a steady point, never longer or shinier than an MG tracer.
const EMBER_EMISSIVE: LinearRgba = LinearRgba {
    red: 10.0,
    green: 2.5,
    blue: 0.5,
    alpha: 1.0,
};
/// Ember point radius (m): a small bead at the base — never the whole shell.
const EMBER_RADIUS: f32 = 0.09;
/// Distance (m) behind the shell's center the ember sits — at its base (the shell's local +Z is the
/// trailing direction, the same axis the tracer streak's tail uses).
const EMBER_BASE_OFFSET: f32 = 0.35;
/// Steady burn time (s) before the ember starts fading — the tracer composition's burn.
const EMBER_BURN: f32 = 2.0;
/// Fade tail (s) after the burn: the ember dies down leaving the smoke trail. (Most shots impact
/// inside the burn window; this only bites on very long-range fire.)
const EMBER_FADE: f32 = 0.4;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, setup_ember_assets)
        .add_systems(Update, (attach_embers, fade_embers));
}

/// Preloaded ember view assets: the shared bead mesh plus the birth-state material VALUE (not a
/// handle — each ember clones it into its own asset so its fade mutates one ember without dimming
/// every other live ember in lockstep). `pub(super)` so the prewarm rig can warm this Blend emissive
/// pipeline at startup.
#[derive(Resource)]
pub(super) struct EmberAssets {
    pub(super) mesh: Handle<Mesh>,
    pub(super) material: StandardMaterial,
}

/// A live ember's age (s); [`fade_embers`] drives the burn-then-fade from it.
#[derive(Component)]
struct Ember {
    age: f32,
}

/// Marks an 88 shell that already carries an ember (so [`attach_embers`] runs once per shell).
#[derive(Component)]
struct Embered;

pub(super) fn setup_ember_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(EmberAssets {
        mesh: meshes.add(Sphere::new(EMBER_RADIUS)),
        // The tracer-streak recipe: black base + zero reflectance so the emissive IS the whole
        // visual, Blend so the fade sinks the glow into the background rather than leaving a black
        // bead (Bevy scales the emissive by the diffuse alpha under Blend).
        material: StandardMaterial {
            base_color: Color::BLACK,
            reflectance: 0.0,
            emissive: EMBER_EMISSIVE,
            alpha_mode: AlphaMode::Blend,
            ..default()
        },
    });
}

/// Give every new 88 shell an ember bead at its base — the same `ShellPath + WorldAssetRoot`
/// signature the trail keys on (only the main-gun branch of `ballistics::on_fire_shell` attaches a
/// scene root), so MG rounds never get one.
fn attach_embers(
    shells: Query<Entity, (With<ShellPath>, With<WorldAssetRoot>, Without<Embered>)>,
    assets: Res<EmberAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    for shell in &shells {
        // Per-ember material asset (clone the template): the fade mutates it every frame; the strong
        // handle lives only on the ember child, so despawning the shell frees the asset.
        let material = materials.add(assets.material.clone());
        commands.entity(shell).insert(Embered).with_child((
            Ember { age: 0.0 },
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(material),
            // The shell's local +Z is the trailing direction; the bead sits a little behind center,
            // at the base.
            Transform::from_translation(Vec3::Z * EMBER_BASE_OFFSET),
            NotShadowCaster,
            NotShadowReceiver,
        ));
    }
}

/// Burn each ember steadily for [`EMBER_BURN`], then fade it over [`EMBER_FADE`] and despawn — the
/// tracer composition running out. (An ember whose shell impacts first despawns with the shell.)
fn fade_embers(
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut embers: Query<(Entity, &mut Ember, &MeshMaterial3d<StandardMaterial>)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (entity, mut ember, material) in &mut embers {
        ember.age += dt;
        let factor = if ember.age < EMBER_BURN {
            1.0
        } else {
            1.0 - (ember.age - EMBER_BURN) / EMBER_FADE
        };
        if factor <= 0.0 {
            // An ember has two lifetime owners — this burn-out and the parent shell's despawn (which
            // recursively despawns the ember child). If the shell impacts and despawns first and the
            // ember's slot is recycled before this despawn lands in the shared command flush, a plain
            // `despawn` would warn on the stale id. `try_despawn` makes the second despawn silent.
            commands.entity(entity).try_despawn();
            continue;
        }
        // Only the fade tail mutates the material. During the steady burn `factor` is a constant
        // 1.0, so the emissive already equals its birth value (each ember owns a fresh clone from
        // `EmberAssets::material`) — writing it every frame would mark the material `Modified` and
        // re-upload it per ember per frame for nothing.
        if ember.age >= EMBER_BURN
            && let Some(mut mat) = materials.get_mut(&material.0)
        {
            mat.emissive = EMBER_EMISSIVE * factor;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The wiring on real ECS systems: an 88-signature shell (ShellPath + WorldAssetRoot) grows one
    /// ember child; an MG round (no scene root) never does; and the ember fades to nothing and
    /// despawns after its burn + fade.
    #[test]
    fn embers_attach_to_the_88_only_and_burn_out() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Time>()
            .add_systems(Update, (attach_embers, fade_embers));
        // Bare ember assets (default handles fine headless).
        app.insert_resource(EmberAssets {
            mesh: Handle::default(),
            material: StandardMaterial {
                emissive: EMBER_EMISSIVE,
                ..default()
            },
        });

        // An 88-signature shell and an MG round (ShellPath only).
        let shell = app
            .world_mut()
            .spawn((ShellPath::default(), WorldAssetRoot::default()))
            .id();
        app.world_mut().spawn(ShellPath::default());
        app.update();

        // Exactly one ember, hung under the 88 shell.
        let world = app.world_mut();
        let embers: Vec<Entity> = world
            .query_filtered::<Entity, With<Ember>>()
            .iter(world)
            .collect();
        assert_eq!(
            embers.len(),
            1,
            "exactly the 88 grows an ember, never the MG"
        );
        assert!(
            app.world()
                .get::<ChildOf>(embers[0])
                .is_some_and(|c| c.parent() == shell),
            "the ember hangs off the 88 shell"
        );

        // Idempotent: a second pass adds no more (the Embered marker gate).
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<Ember>>()
                .iter(world)
                .count(),
            1
        );

        // Burn steady, then fade out and despawn.
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(
                EMBER_BURN + EMBER_FADE + 0.1,
            ));
        app.update();
        let world = app.world_mut();
        assert_eq!(
            world
                .query_filtered::<Entity, With<Ember>>()
                .iter(world)
                .count(),
            0,
            "a burned-out ember despawns"
        );
    }
}
