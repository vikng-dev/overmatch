//! Test-only: whether a built schedule orders one node ahead of another.

use bevy::ecs::schedule::{NodeId, ScheduleGraph};

/// Whether `graph` orders `first` ahead of `then`: a path of ordering edges, stepping from a set
/// into its members (an edge declared on a set binds every member). Run after the schedule is
/// built; a node bevy created from a `.before`/`.after` reference alone has no members, so
/// name the system when the order is about a system.
pub(crate) fn orders(graph: &ScheduleGraph, first: NodeId, then: NodeId) -> bool {
    let (dependency, hierarchy) = (graph.dependency().graph(), graph.hierarchy().graph());
    let (mut stack, mut seen) = (vec![first], Vec::new());
    while let Some(at) = stack.pop() {
        if at == then {
            return true;
        }
        if !seen.contains(&at) {
            seen.push(at);
            stack.extend(dependency.neighbors(at));
            stack.extend(hierarchy.neighbors(at));
        }
    }
    false
}
