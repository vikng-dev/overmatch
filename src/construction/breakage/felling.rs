//! Felling a standing construction at its ground supports, about the game's environment
//! destruction (game state only): one zero-length line on each support, delivered as a march step
//! of a shell that leaves no mark, so the ordinary deposit, law and launch paths perform the break.
//! It fires no round; instruments use it to break a construction on schedule.

use std::collections::BTreeSet;

use bevy::prelude::*;

use super::Breakage;
use crate::ballistics::{
    BallisticVolume, DeliveredLine, PenetrationMarks, ShellMarched, VolumeCrossing,
};
use crate::construction::Element;

/// The chunks of `body` that hold it to the ground: buried or anchored.
pub(crate) fn supports(body: &Breakage) -> BTreeSet<usize> {
    let anchored: BTreeSet<_> = body.kind.asset.anchors.iter().map(|a| a.chunk).collect();
    body.live
        .structure
        .chunks
        .iter()
        .enumerate()
        .filter(|(c, chunk)| chunk.buried || anchored.contains(&body.kind.layout.chunks[*c]))
        .map(|(c, _)| c)
        .collect()
}

/// One zero-length line on each of `supports`, chunks of the standing construction `id` at
/// `root` posed `pose`, carrying enough energy to remove it through the ordinary deposit path.
pub(crate) fn felling_lines(
    id: &str,
    root: Entity,
    pose: &Transform,
    body: &Breakage,
    supports: &BTreeSet<usize>,
    volumes: &Query<(Entity, &ChildOf, &Element), With<BallisticVolume>>,
) -> Vec<DeliveredLine> {
    supports
        .iter()
        .map(|&c| {
            let chunk = &body.live.structure.chunks[c];
            assert!(
                body.live.state.standing(c),
                "a support to fell must still stand; instance {id} chunk {c}"
            );
            let element = body.kind.layout.element_of[c];
            let volume = volumes
                .iter()
                .find(|(_, parent, e)| parent.parent() == root && e.0 == element)
                .expect("an untouched support retains its element volume")
                .0;
            let at = pose.translation + pose.rotation * chunk.centroid;
            // DERIVED: twice V * crush exceeds this scaled chunk's complete work pool.
            // A zero-length line deposits at its centroid, without inventing a projectile.
            let energy = 2.0 * chunk.volume * chunk.material.crush;
            DeliveredLine {
                crossing: VolumeCrossing {
                    volume,
                    entry: at,
                    end: at,
                    chord: 0.0,
                    cost: 0.0,
                    energy,
                    perforated: true,
                },
                contacts: vec![],
                momentum: Vec3::ZERO,
                entrance_momentum: Vec3::ZERO,
                entrance_energy: 0.0,
            }
        })
        .collect()
}

/// Deliver `lines` as one march step of a shell that leaves no mark.
pub(crate) fn fell(commands: &mut Commands, lines: Vec<DeliveredLine>) {
    let shell = commands.spawn(PenetrationMarks::default()).id();
    commands.trigger(ShellMarched {
        shell,
        marks_from: 0,
        cones_from: 0,
        burst: None,
        lines,
    });
    commands.entity(shell).despawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Felling must remove buried supports as well as anchor faces: the scheduled workload and
    /// the frozen-rubble bed rely on it, and leaving either ground path behind changes a requested
    /// collapse into a partial break.
    #[test]
    fn support_selection_includes_buried_matter_without_an_anchor_face() {
        let kind = crate::construction::tests::column_construction();
        let mut body = Breakage::new(kind, 0);
        // Fixture: a second ground-held chunk has no authored anchor face.
        let extra = body.live.structure.chunks.len() - 1;
        std::sync::Arc::make_mut(&mut body.live.structure).chunks[extra].buried = true;
        let selected = supports(&body);
        assert!(selected.contains(&extra));
        for anchor in &body.kind.asset.anchors {
            let law = body
                .kind
                .layout
                .chunks
                .iter()
                .position(|&c| c == anchor.chunk)
                .unwrap();
            assert!(
                selected.contains(&law),
                "authored anchors also support the construction"
            );
        }
    }
}
