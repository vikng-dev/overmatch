//! Network-only admission for the two parent-local angles carried by ServoAngles.
//! Static ancestors compose locally; any other moving ancestor needs a stream the game lacks.

use std::collections::BTreeSet;

use bevy::prelude::*;

use crate::bake::TankBlueprint;
use crate::spec::ViewKind;

pub(super) fn plugin(app: &mut App) {
    // Startup's bake publishes the blueprint before this gate; gameplay opens in Update.
    app.add_systems(PostStartup, admit);
}

fn admit(blueprint: Res<TankBlueprint>) {
    let spec = &blueprint.spec;
    let (Some(yaw), Some(pitch)) = (
        spec.gunner_yaw.as_deref(),
        spec.views
            .get(&ViewKind::Gunner)
            .map(|view| view.node.as_str()),
    ) else {
        panic!(
            "{}: gunner_yaw / views[Gunner].node: ServoAngles requires both aim handles; author a playable Gunner rig before network play",
            blueprint.file
        );
    };
    let moving: BTreeSet<_> = spec.articulated_nodes().collect();
    let geometry = &blueprint.geometry;
    for (field, axis) in [("gunner_yaw", yaw), ("views[Gunner].node", pitch)] {
        let index = geometry.by_name[axis];
        let mut parent = geometry.nodes[index].parent;
        while let Some(index) = parent {
            let node = &geometry.nodes[index];
            assert!(
                node.name == yaw || node.name == pitch || !moving.contains(node.name.as_str()),
                "{}: {field} {axis:?}: moving ancestor {:?} is absent from ServoAngles; parent this aim chain only through static frames and its streamed yaw/pitch before network play",
                blueprint.file,
                node.name
            );
            parent = node.parent;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bake::{NodeGeometry, TankGeometry};

    /// Remote clients can compose static construction but cannot invent an ancestor's motion.
    /// Mount the production admission plugin: dropping that gate admits these concealed axes.
    #[test]
    fn network_admission_refuses_unstreamed_moving_ancestors_of_either_axis() {
        for (concealed, servo) in [None, Some("yaw"), Some("pitch")]
            .into_iter()
            .flat_map(|axis| [false, true].map(|servo| (axis, servo)))
        {
            let blueprint = fixture(concealed, servo);
            let mut app = App::new();
            app.insert_resource(blueprint).add_plugins(plugin);
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.update()));
            if concealed.is_none() {
                result.unwrap();
            } else {
                let error = result.unwrap_err();
                let message = error.downcast_ref::<String>().unwrap();
                assert!(
                    message.contains("fixture.tank.ron")
                        && message.contains("moving ancestor")
                        && message.contains("ServoAngles")
                        && message.contains("before network play"),
                    "{message}"
                );
            }
        }
    }

    fn fixture(concealed: Option<&str>, servo: bool) -> TankBlueprint {
        // Fixture sheet supplies declarations; the inline node table supplies only this gate's
        // hierarchy. No vehicle's geometry, identity or authored magnitudes are pass conditions.
        let mut spec: crate::spec::TankSpec = crate::provenance::from_shipped_str(
            "fixture.tank.ron",
            include_str!(concat!(
                "../../assets/",
                crate::assets::tank_file!(".tank.ron")
            )),
        )
        .unwrap();
        let moving = if servo {
            let name = "fixture hidden yaw";
            spec.servos.insert(
                name.into(),
                ron::from_str(
                    r#"(
                role: Yaw,
                drive: Hand(hand_rate: (1.0, Fixture("fixture angular rate"))),
                travel: Continuous,
                requires: [Gunner],
            )"#,
                )
                .unwrap(),
            );
            name.to_owned()
        } else {
            spec.roadwheels[0].node.clone()
        };
        let yaw = spec.gunner_yaw.as_ref().unwrap();
        let pitch = &spec.views[&ViewKind::Gunner].node;
        let node = |name: &str, parent| NodeGeometry {
            name: name.into(),
            parent,
            transform: Transform::IDENTITY,
            root_position: Vec3::ZERO,
            root_rotation: Quat::IDENTITY,
            root_scale: Vec3::ONE,
            primitives: vec![],
        };
        let nodes = vec![
            node("fixture root", None),
            node("fixture static frame", Some(0)),
            node(yaw, Some(if concealed == Some("yaw") { 4 } else { 1 })),
            node(pitch, Some(if concealed == Some("pitch") { 4 } else { 2 })),
            node(
                &moving,
                Some(if concealed == Some("pitch") { 2 } else { 1 }),
            ),
        ];
        let by_name = nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.name.clone(), i))
            .collect();
        TankBlueprint::new(
            "fixture.tank.ron".into(),
            TankGeometry {
                nodes,
                by_name,
                roadwheels: vec![],
                collision_proxies: vec![],
                ballistic_volumes: vec![],
            },
            spec,
        )
    }
}
