//! Island waking by contact: only an awake body wakes a resting island. A contact that starts
//! generating constraints between sides that are each at rest (a body in a sleeping island) or
//! cannot move (a static or disabled body, which holds no island node) supplies no wake: the pass
//! links it to the island it joins, merging the resting islands it bridges, and puts the pair to
//! sleep with them, so it leaves no constraint for the solver. The island's next wake brings the
//! pair back and its manifolds into the solver.
//!
//! Avian wakes every sleeping island a new contact links, a resting body's new stationary support
//! included. The pass runs in `NarrowPhaseSystems::Last`, after the narrow phase has queued the
//! step's status changes and before the solver applies them, and takes each such contact's entry
//! out of the queue. A merge it makes stands when a later entry wakes the merged island: the woken
//! island holds the pair. Who relies on it: the server's resting scenery and debris, which a
//! stationary contact must leave asleep.

use avian3d::collision::narrow_phase::{
    ContactStatusChange, ContactStatusChangeQueue, NarrowPhaseSystems,
};
use avian3d::dynamics::solver::islands::{BodyIslandNode, PhysicsIslands};
use avian3d::prelude::*;
use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.get_schedule_mut(PhysicsSchedule)
        .expect("avian_ext::plugin is added after Avian's PhysicsPlugins")
        .add_systems(
            link_resting_contacts
                .in_set(NarrowPhaseSystems::Last)
                .after(super::contact_reduction::reduce_contacts),
        );
}

/// Links each queued new contact between resting sides to its island without a wake, puts its pair
/// to sleep and takes its entry out of the queue (see the module doc).
pub(crate) fn link_resting_contacts(
    mut queue: ResMut<ContactStatusChangeQueue>,
    mut graph: ResMut<ContactGraph>,
    mut islands: ResMut<PhysicsIslands>,
    mut body_islands: Query<&mut BodyIslandNode, Or<(With<Disabled>, Without<Disabled>)>>,
) {
    queue.retain(|change| {
        let &ContactStatusChange::StartedGeneratingConstraints(contact) = change else {
            return true;
        };
        // A pair removed after the narrow phase queued it: Avian's flush skips the entry.
        let Some(edge) = graph.get_edge_by_id(contact) else {
            return true;
        };
        // Per side: `Some(resting)` for a body in an island, `None` for a side that holds none.
        let sides = [edge.body1, edge.body2].map(|body| {
            let node = body.and_then(|body| body_islands.get(body).ok())?;
            let island = islands
                .get(node.island_id())
                .expect("a body's island node names a live island");
            Some(island.is_sleeping())
        });
        let resting = sides.contains(&Some(true)) && !sides.contains(&Some(false));
        if !resting {
            return true;
        }
        let collider = edge.collider1;
        let linked = islands
            .add_contact(contact, &mut body_islands, &graph)
            .expect("a side in an island links the contact");
        assert!(
            linked.is_sleeping(),
            "merging resting islands leaves the island resting"
        );
        graph.sleep_entity_with(collider, |_, _| {}, |edge, _| edge.id == contact);
        false
    });
}
