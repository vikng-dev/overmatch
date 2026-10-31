//! The game's damage-model view: a round's authored tracer ember.
//!
//! Placement, radius, linear HDR colour and burn belong to the round's required emission record.
//! The shared renderer attaches a bead only when that record declares one, independently of the
//! projectile's scene or calibre. The bead follows the shell and expires with either its burn or
//! its parent. Nothing in this module writes simulation state (ADR-0014).

use crate::rounds::{EmberEmission, FlightEffects};
use bevy::light::{NotShadowCaster, NotShadowReceiver};
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, setup_ember_assets)
        .add_observer(attach_ember)
        .add_systems(Update, fade_embers);
}

/// Shared unit geometry and Blend material recipe. Each emitter owns its material so a fade
/// never dims another round. Prewarming uses this same geometry and pipeline recipe.
#[derive(Resource)]
pub(super) struct EmberAssets {
    pub(super) mesh: Handle<Mesh>,
    pub(super) material: StandardMaterial,
}

#[derive(Component)]
struct Ember {
    age: f32,
    emission: EmberEmission,
}

#[derive(Component)]
struct Embered;

pub(super) fn setup_ember_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(EmberAssets {
        mesh: meshes.add(Sphere::new(1.0)),
        material: StandardMaterial {
            base_color: Color::BLACK,
            reflectance: 0.0,
            alpha_mode: AlphaMode::Blend,
            ..default()
        },
    });
}

fn glow(emission: &EmberEmission) -> LinearRgba {
    LinearRgba::rgb(
        emission.emissive[0],
        emission.emissive[1],
        emission.emissive[2],
    )
}

/// An explicit absent emitter grows no bead. Re-adding the record retains the existing child.
fn attach_ember(
    add: On<Add, FlightEffects>,
    rounds: Query<(&FlightEffects, Has<Embered>)>,
    assets: Res<EmberAssets>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let (effects, attached) = rounds
        .get(add.entity)
        .expect("the Add observer reads the complete round emission bundle");
    if attached {
        return;
    }
    let Some(emission) = effects.ember else {
        return;
    }; // Authored untraced ammunition.
    let mut material = assets.material.clone();
    material.emissive = glow(&emission);
    let material = materials.add(material);
    commands
        .entity(add.entity)
        .insert((Embered, Visibility::Inherited))
        .with_child((
            Ember { age: 0.0, emission },
            Mesh3d(assets.mesh.clone()),
            MeshMaterial3d(material),
            Transform::from_translation(Vec3::Z * emission.base_offset_m)
                .with_scale(Vec3::splat(emission.radius_m)),
            NotShadowCaster,
            NotShadowReceiver,
        ));
}

fn fade_embers(
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut embers: Query<(Entity, &mut Ember, &MeshMaterial3d<StandardMaterial>)>,
    mut commands: Commands,
) {
    for (entity, mut ember, material) in &mut embers {
        ember.age += time.delta_secs();
        let factor = if ember.age < ember.emission.burn_s {
            1.0
        } else {
            1.0 - (ember.age - ember.emission.burn_s) / ember.emission.fade_s
        };
        if factor <= 0.0 {
            // Parent expiry and this burn-out can share a command flush; either may despawn first.
            commands.entity(entity).try_despawn();
            continue;
        }
        if ember.age >= ember.emission.burn_s {
            let mut material = materials
                .get_mut(&material.0)
                .expect("the live ember holds a strong handle to its private material");
            material.emissive = glow(&ember.emission) * factor;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A round's emission controls presence, placement, radius, colour and burn. Viewers rely on
    /// untraced ammunition staying untraced and different constructions retaining their own facts.
    #[test]
    fn embers_follow_authored_presence_and_burn() {
        let mut app = App::new();
        app.init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Time>()
            .add_observer(attach_ember)
            .add_systems(Update, fade_embers);
        app.insert_resource(EmberAssets {
            mesh: Handle::default(),
            material: StandardMaterial::default(),
        });
        let facts = FlightEffects::fixture_traced();
        let shell = app.world_mut().spawn(facts).id();
        let untraced = app.world_mut().spawn(FlightEffects::NONE).id();
        let mut other = facts;
        let emission = other.ember.as_mut().unwrap();
        emission.radius_m *= 2.0;
        emission.base_offset_m *= 3.0;
        emission.burn_s = 0.1;
        emission.fade_s = 0.1;
        emission.emissive = [1.0, 0.0, 0.0];
        let second = app.world_mut().spawn(other).id();
        app.update();
        let world = app.world_mut();
        let beads: Vec<_> = world
            .query::<(
                &ChildOf,
                &Transform,
                &Ember,
                &MeshMaterial3d<StandardMaterial>,
            )>()
            .iter(world)
            .map(|(parent, pose, ember, material)| {
                (parent.parent(), *pose, ember.emission, material.0.clone())
            })
            .collect();
        assert_eq!(beads.len(), 2);
        assert!(!beads.iter().any(|(owner, ..)| *owner == untraced));
        for (owner, pose, emission, material) in &beads {
            assert_eq!(pose.translation, Vec3::Z * emission.base_offset_m);
            assert_eq!(pose.scale, Vec3::splat(emission.radius_m));
            assert_eq!(
                world
                    .resource::<Assets<StandardMaterial>>()
                    .get(material)
                    .unwrap()
                    .emissive,
                glow(emission)
            );
            assert!(*owner == shell || *owner == second);
        }
        app.world_mut().entity_mut(shell).remove::<FlightEffects>();
        app.world_mut().entity_mut(shell).insert(facts);
        app.update();
        assert_eq!(
            app.world_mut().query::<&Ember>().iter(app.world()).count(),
            2
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(0.3));
        app.update();
        assert_eq!(
            app.world_mut().query::<&Ember>().iter(app.world()).count(),
            1
        );
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(3.0));
        app.update();
        assert_eq!(
            app.world_mut().query::<&Ember>().iter(app.world()).count(),
            0
        );
    }
}
