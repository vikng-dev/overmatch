//! Collider admission for the game's running-gear contact interface (game state only).

use avian3d::prelude::{
    Collider, CollisionLayers, LayerMask, PhysicsSchedule, PhysicsStepSystems, Sensor,
};
use bevy::prelude::*;

use crate::Layer;
use crate::state::SimPhase;
use crate::track::terrain::FieldSurface;

/// The belt uses its hull's physical response class; sensors and query-only volumes are not matter.
pub(crate) fn solid(layers: &CollisionLayers, sensor: bool) -> bool {
    !sensor && layers.interacts_with(CollisionLayers::new([Layer::Vehicle], LayerMask::ALL))
}

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(admit_inserted)
        .add_systems(PreUpdate, admit_changed)
        .add_systems(FixedUpdate, admit_changed.before(SimPhase::DrivingForces))
        // Avian may update scaled geometry in Prepare; refuse it before broad/narrow phase.
        .add_systems(
            PhysicsSchedule,
            admit_changed.in_set(PhysicsStepSystems::First),
        );
}

type Sources<'w, 's> = Query<'w, 's, (Option<&'static Name>, Option<&'static ChildOf>)>;
type Candidate<'a> = (Entity, &'a Collider, &'a CollisionLayers, bool, bool);

fn refuse_unsupported(candidate: Candidate<'_>, sources: &Sources) {
    let (entity, collider, layers, sensor, field) = candidate;
    if !solid(layers, sensor) {
        return; // Sensors and query-only volumes are observations, not physical matter.
    }
    let supported = |shape: &Collider| {
        let shape = shape.shape_scaled();
        if field {
            shape.as_cuboid().is_some() || shape.as_heightfield().is_some()
        } else {
            shape.as_cuboid().is_some() || shape.as_convex_polyhedron().is_some()
        }
    };
    if supported(collider) {
        return;
    }
    // Names on the collider or its ancestors identify the authored instance and source field.
    // Unnamed procedural entities still print their identity; they have no asset source name.
    let mut source = Vec::new();
    let mut current = entity;
    loop {
        let (name, parent) = sources
            .get(current)
            .expect("an admitted collider and its ancestors are alive");
        source.push(match name {
            Some(name) => format!("{} ({current:?})", name.as_str()),
            None => format!("unnamed entity {current:?}"),
        });
        match parent {
            Some(parent) => current = parent.parent(),
            None => break,
        }
    }
    panic!(
        "world contact: source {}, Collider.shape {:?} has no {} adapter; use supported geometry or implement an adapter before publishing this solid",
        source.join(" <- "),
        collider.shape_scaled().shape_type(),
        if field {
            "field (box or height grid)"
        } else {
            "collider (box or convex polyhedron)"
        },
    );
}

fn admit_inserted(
    inserted: On<Insert, (Collider, CollisionLayers, FieldSurface)>,
    colliders: Query<(
        Entity,
        &Collider,
        &CollisionLayers,
        Has<Sensor>,
        Has<FieldSurface>,
    )>,
    sources: Sources,
) {
    if let Ok(candidate) = colliders.get(inserted.entity) {
        refuse_unsupported(candidate, &sources);
    } // A layer can be inserted before its collider; it is not geometry yet.
}

/// In-place shape/layer edits and removal of observation semantics must pass the same gate.
fn admit_changed(
    changed: Query<
        (
            Entity,
            &Collider,
            &CollisionLayers,
            Has<Sensor>,
            Has<FieldSurface>,
        ),
        Or<(Changed<Collider>, Changed<CollisionLayers>)>,
    >,
    colliders: Query<(
        Entity,
        &Collider,
        &CollisionLayers,
        Has<Sensor>,
        Has<FieldSurface>,
    )>,
    mut sensors: RemovedComponents<Sensor>,
    mut fields: RemovedComponents<FieldSurface>,
    sources: Sources,
) {
    changed
        .par_iter()
        .for_each(|candidate| refuse_unsupported(candidate, &sources));
    // A despawned collider has no world-contact obligation; the many-query skips it.
    colliders
        .par_iter_many(sensors.read().chain(fields.read()))
        .for_each(|candidate| refuse_unsupported(candidate, &sources));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Admission happens while data is published, without a tank or a proximity query.
    /// Mount the shipped sim installer so removing its admission registration breaks this witness.
    #[test]
    #[should_panic(expected = "fixtures/world.ron::solids[0]")]
    fn unsupported_geometry_is_refused_at_publication_with_its_source() {
        let mut app = App::new();
        app.add_plugins(crate::track::sim::sim_plugin);
        let source = app
            .world_mut()
            .spawn(Name::new("fixtures/world.ron::solids[0]"))
            .id();
        // Fixture: a ball has no world-contact adapter, even arbitrarily far from any tank.
        app.world_mut()
            .spawn((ChildOf(source), Collider::sphere(1.0)));
    }

    /// Unsupported observation geometry is legal; changing its response makes it matter.
    #[test]
    #[should_panic(expected = "Collider.shape Ball")]
    fn changing_query_only_geometry_to_a_solid_requires_an_adapter() {
        let mut app = App::new();
        app.add_plugins(plugin);
        let query = app
            .world_mut()
            .spawn((
                Name::new("fixtures/world.ron::query"),
                Collider::sphere(1.0),
                CollisionLayers::new(LayerMask::ALL, LayerMask::NONE),
            ))
            .id();
        app.world_mut()
            .entity_mut(query)
            .insert(CollisionLayers::default());
    }

    /// Mutable collider replacement must be refused before any force or physics consumer runs.
    #[test]
    #[should_panic(expected = "Collider.shape Ball")]
    fn in_place_shape_edits_pass_the_publication_gate() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, plugin));
        let solid = app.world_mut().spawn(Collider::cuboid(1.0, 1.0, 1.0)).id();
        app.update();
        *app.world_mut().get_mut::<Collider>(solid).unwrap() = Collider::sphere(1.0);
        app.update();
    }

    /// Removing sensor semantics makes its geometry physical even without a shape change.
    #[test]
    #[should_panic(expected = "Collider.shape Ball")]
    fn removing_sensor_semantics_requires_a_physical_adapter() {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, plugin));
        let sensor = app.world_mut().spawn((Collider::sphere(1.0), Sensor)).id();
        app.update();
        app.world_mut().entity_mut(sensor).remove::<Sensor>();
        app.update();
    }

    /// Neither observation class is admitted, even if its shape is otherwise unsupported.
    #[test]
    fn sensors_and_query_only_geometry_need_no_physical_adapter() {
        let mut app = App::new();
        app.add_plugins(plugin);
        app.world_mut().spawn((Collider::sphere(1.0), Sensor));
        app.world_mut().spawn((
            Collider::sphere(1.0),
            CollisionLayers::new(LayerMask::ALL, LayerMask::NONE),
        ));
    }
}
